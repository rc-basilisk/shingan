use crate::detector::Detector;
use crate::file_info::{FileCategory, FileInfo};
use crate::scanner::grouping::{self, DuplicateGroup};
use crate::scanner::FileScanner;
use crossbeam_channel::Sender;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Files smaller than this are ignored by default (1 KiB).
pub const DEFAULT_MIN_SIZE: u64 = 1024;

/// A freshly computed signature, with the file metadata needed to key a cache entry.
#[derive(Debug, Clone)]
pub struct NewSignature {
    pub path: String,
    pub size: u64,
    /// Modification time in whole seconds since the Unix epoch.
    pub modified_secs: i64,
    pub category: FileCategory,
    pub signature: String,
}

/// Duplicate groups keyed by category, plus newly computed signatures to persist.
pub type ScanOutcome = (
    HashMap<FileCategory, Vec<DuplicateGroup>>,
    Vec<NewSignature>,
);

/// A persistent store of previously computed signatures (e.g. the SQLite
/// `signature_cache` table). Implementations must only return a signature
/// when the file's size and modification time still match.
pub trait SignatureCache: Send + Sync {
    /// Look up cached signatures; returns `path -> signature` for hits.
    fn lookup(&self, category: FileCategory, files: &[&FileInfo]) -> HashMap<String, String>;
}

/// Lets a shared `Arc<impl SignatureCache>` (e.g. `Arc<Database>`) be used directly.
impl<T: SignatureCache + ?Sized> SignatureCache for Arc<T> {
    fn lookup(&self, category: FileCategory, files: &[&FileInfo]) -> HashMap<String, String> {
        self.as_ref().lookup(category, files)
    }
}

/// Modification time of a file in whole seconds since the Unix epoch.
pub fn modified_secs(info: &FileInfo) -> i64 {
    info.modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Progress messages sent from the scanner to the UI.
#[derive(Debug, Clone)]
pub enum ScanProgress {
    Status(String),
    Progress {
        current: u32,
        total: u32,
        message: String,
        elapsed_secs: f64,
        eta_secs: Option<f64>,
    },
    PhaseCompleted {
        category: FileCategory,
        groups: Vec<DuplicateGroup>,
    },
    Completed,
    Error(String),
}

/// Controls for pausing/stopping a running scan.
pub struct ScanControl {
    pub paused: AtomicBool,
    pub stopped: AtomicBool,
}

impl ScanControl {
    pub fn new() -> Self {
        Self {
            paused: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        }
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Block while paused, return false if stopped.
    pub fn wait_if_paused(&self) -> bool {
        while self.is_paused() {
            if self.is_stopped() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        !self.is_stopped()
    }
}

impl Default for ScanControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Orchestrates the 3-phase duplicate scanning process.
pub struct DuplicateScanner {
    detectors: HashMap<FileCategory, Box<dyn Detector>>,
    file_scanner: FileScanner,
    control: Arc<ScanControl>,
    similarity_threshold: f64,
    progress_tx: Sender<ScanProgress>,
    /// Persistent signature store consulted before computing signatures.
    cache: Option<Box<dyn SignatureCache>>,
    /// Worker thread count (0 = one per CPU core).
    threads: usize,
}

impl DuplicateScanner {
    pub fn new(
        categories: &[FileCategory],
        detectors: HashMap<FileCategory, Box<dyn Detector>>,
        similarity_threshold: f64,
        control: Arc<ScanControl>,
        progress_tx: Sender<ScanProgress>,
    ) -> Self {
        Self {
            detectors,
            file_scanner: FileScanner::new(categories).with_size_limits(DEFAULT_MIN_SIZE, None),
            control,
            similarity_threshold,
            progress_tx,
            cache: None,
            threads: 0,
        }
    }

    /// Use a persistent signature cache so unchanged files are not re-analyzed.
    pub fn with_signature_cache(mut self, cache: Box<dyn SignatureCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Ignore files smaller than `min` bytes (default 1 KiB) or larger than `max`.
    pub fn with_size_limits(mut self, min: u64, max: Option<u64>) -> Self {
        self.file_scanner = self.file_scanner.with_size_limits(min, max);
        self
    }

    /// Number of worker threads for analysis (0 = one per CPU core).
    pub fn with_threads(mut self, threads: usize) -> Self {
        self.threads = threads;
        self
    }

    fn send(&self, msg: ScanProgress) {
        let _ = self.progress_tx.send(msg);
    }

    /// Run the full 3-phase scan on the given paths.
    ///
    /// Each path is a (directory, include_subdirs) tuple.
    /// Returns (duplicate groups by category, newly computed signatures).
    /// The new signatures are `(path_string, signature)` pairs that the caller
    /// should persist to the signature cache for future rescans.
    pub fn scan_paths(&self, paths: &[(PathBuf, bool)]) -> ScanOutcome {
        if self.threads > 0 {
            match rayon::ThreadPoolBuilder::new()
                .num_threads(self.threads)
                .build()
            {
                Ok(pool) => return pool.install(|| self.scan_paths_inner(paths)),
                Err(e) => self.send(ScanProgress::Status(format!(
                    "Could not create a {}-thread pool ({e}); using defaults",
                    self.threads
                ))),
            }
        }
        self.scan_paths_inner(paths)
    }

    fn scan_paths_inner(&self, paths: &[(PathBuf, bool)]) -> ScanOutcome {
        let mut all_results: HashMap<FileCategory, Vec<DuplicateGroup>> = HashMap::new();
        let mut all_new_signatures: Vec<NewSignature> = Vec::new();

        self.send(ScanProgress::Status(
            "Phase 1/3: Discovering files...".to_string(),
        ));

        let mut files_by_category: HashMap<FileCategory, Vec<FileInfo>> = HashMap::new();
        let mut total_skipped_permission: u32 = 0;
        let mut total_skipped_other: u32 = 0;

        for (path, include_subdirs) in paths {
            if self.control.is_stopped() {
                return (all_results, all_new_signatures);
            }

            let result = self
                .file_scanner
                .scan_directory(path, *include_subdirs, None);
            total_skipped_permission += result.skipped_permission;
            total_skipped_other += result.skipped_other;

            for file in result.files {
                files_by_category
                    .entry(file.category)
                    .or_default()
                    .push(file);
            }
        }

        let total_files: usize = files_by_category.values().map(|v| v.len()).sum();
        let skipped_total = total_skipped_permission + total_skipped_other;
        if skipped_total > 0 {
            self.send(ScanProgress::Status(format!(
                "Phase 1/3: Found {} files ({} skipped: {} permission denied, {} other errors)",
                total_files, skipped_total, total_skipped_permission, total_skipped_other
            )));
        } else {
            self.send(ScanProgress::Status(format!(
                "Phase 1/3: Found {} files",
                total_files
            )));
        }

        // Phase 2: Analysis (per category), in a stable order
        let mut categories: Vec<FileCategory> = files_by_category.keys().copied().collect();
        categories.sort_by_key(|c| FileCategory::all().iter().position(|x| x == c));

        for category in &categories {
            if self.control.is_stopped() {
                return (all_results, all_new_signatures);
            }

            let files = match files_by_category.get(category) {
                Some(f) if !f.is_empty() => f,
                _ => continue,
            };

            let detector = match self.detectors.get(category) {
                Some(d) => d,
                None => continue,
            };

            self.send(ScanProgress::Status(format!(
                "Phase 2/3: Analyzing {}s ({} files)...",
                category.label(),
                files.len()
            )));

            let (groups, new_sigs) = self.find_duplicates(files, detector.as_ref());
            all_new_signatures.extend(new_sigs);

            if !groups.is_empty() {
                self.send(ScanProgress::PhaseCompleted {
                    category: *category,
                    groups: groups.clone(),
                });
                all_results.insert(*category, groups);
            }
        }

        self.send(ScanProgress::Completed);
        (all_results, all_new_signatures)
    }

    /// Phase 2: Compute signatures and find duplicate groups for a single category.
    /// Returns the groups plus newly computed signatures for the caller to persist.
    fn find_duplicates(
        &self,
        files: &[FileInfo],
        detector: &dyn Detector,
    ) -> (Vec<DuplicateGroup>, Vec<NewSignature>) {
        // Exact-only detectors: a file with a unique size cannot have a duplicate.
        let files: Vec<&FileInfo> = if detector.exact_only() {
            let mut by_size: HashMap<u64, usize> = HashMap::new();
            for f in files {
                *by_size.entry(f.size).or_default() += 1;
            }
            files.iter().filter(|f| by_size[&f.size] > 1).collect()
        } else {
            files.iter().collect()
        };
        if files.len() < 2 {
            return (Vec::new(), Vec::new());
        }

        let cached = match &self.cache {
            Some(cache) => cache.lookup(detector.category(), &files),
            None => HashMap::new(),
        };

        // 2a: Compute signatures in parallel using rayon, using cache when available
        let control = self.control.clone();
        let file_count = files.len() as u32;
        let completed = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let phase_start = Instant::now();

        // (file, signature_option, was_cache_hit)
        let signatures: Vec<(&FileInfo, Option<String>, bool)> = files
            .par_iter()
            .filter(|_| !control.is_stopped())
            .map(|&file| {
                if !control.wait_if_paused() {
                    return (file, None, false);
                }

                let path_str = file.path.to_string_lossy();
                let (sig, from_cache) = if let Some(cached_sig) = cached.get(path_str.as_ref()) {
                    (Some(cached_sig.clone()), true)
                } else {
                    // A panicking decoder must not take the whole scan down.
                    let sig = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        detector.compute_signature(&file.path)
                    }))
                    .ok()
                    .and_then(|r| r.ok())
                    .flatten();
                    (sig, false)
                };

                let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                if done.is_multiple_of(10) || done == file_count {
                    let elapsed = phase_start.elapsed().as_secs_f64();
                    let eta = if done > 0 && done < file_count {
                        Some(elapsed / done as f64 * (file_count - done) as f64)
                    } else {
                        None
                    };
                    let pct = (done as f64 / file_count as f64 * 100.0) as u32;
                    let _ = self.progress_tx.send(ScanProgress::Progress {
                        current: done,
                        total: file_count,
                        message: format!(
                            "Analyzing {}s: {}/{} ({}%)",
                            detector.category().label(),
                            done,
                            file_count,
                            pct
                        ),
                        elapsed_secs: elapsed,
                        eta_secs: eta,
                    });
                }

                (file, sig, from_cache)
            })
            .collect();

        if self.control.is_stopped() {
            return (Vec::new(), Vec::new());
        }

        // Collect newly computed signatures for the caller to persist
        let new_signatures: Vec<NewSignature> = signatures
            .iter()
            .filter(|(_, _, from_cache)| !from_cache)
            .filter_map(|(file, sig, _)| {
                sig.as_ref().map(|s| NewSignature {
                    path: file.path.to_string_lossy().into_owned(),
                    size: file.size,
                    modified_secs: modified_secs(file),
                    category: file.category,
                    signature: s.clone(),
                })
            })
            .collect();

        let cache_hits: usize = signatures.iter().filter(|(_, _, hit)| *hit).count();
        let failed: usize = signatures.iter().filter(|(_, s, _)| s.is_none()).count();

        // Build signature map (only successful signatures)
        let file_signatures: HashMap<PathBuf, String> = signatures
            .into_iter()
            .filter_map(|(file, sig, _)| sig.map(|s| (file.path.clone(), s)))
            .collect();

        let mut summary = format!(
            "Phase 3/3: Comparing {} {} signatures",
            file_signatures.len(),
            detector.category().label()
        );
        if cache_hits > 0 {
            summary.push_str(&format!(" ({cache_hits} from cache)"));
        }
        if failed > 0 {
            summary.push_str(&format!(" — {failed} unreadable files skipped"));
        }
        self.send(ScanProgress::Status(summary));

        // Find duplicate groups.
        let category = detector.category();
        let phase3_start = Instant::now();
        let all_groups = match category {
            FileCategory::Image
            | FileCategory::Video
            | FileCategory::Document
            | FileCategory::Code => {
                let progress_tx = self.progress_tx.clone();
                grouping::find_fuzzy_groups(
                    &file_signatures,
                    detector,
                    self.similarity_threshold,
                    Some(&|done, total| {
                        let elapsed = phase3_start.elapsed().as_secs_f64();
                        let eta = if done > 0 && done < total {
                            Some(elapsed / done as f64 * (total - done) as f64)
                        } else {
                            None
                        };
                        let pct = if total > 0 {
                            (done as f64 / total as f64 * 100.0) as u32
                        } else {
                            0
                        };
                        let _ = progress_tx.send(ScanProgress::Progress {
                            current: done as u32,
                            total: total as u32,
                            message: format!("Comparing signatures: {}/{} ({}%)", done, total, pct),
                            elapsed_secs: elapsed,
                            eta_secs: eta,
                        });
                    }),
                    Some(&self.control),
                )
            }
            FileCategory::Archive => grouping::find_exact_groups(&file_signatures),
        };

        (all_groups, new_signatures)
    }
}
