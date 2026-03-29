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
use std::time::Duration;

/// Progress messages sent from the scanner to the UI.
#[derive(Debug, Clone)]
pub enum ScanProgress {
    Status(String),
    Progress {
        current: u32,
        total: u32,
        message: String,
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
            file_scanner: FileScanner::new(categories).with_size_limits(1024, None),
            control,
            similarity_threshold,
            progress_tx,
        }
    }

    fn send(&self, msg: ScanProgress) {
        let _ = self.progress_tx.send(msg);
    }

    /// Run the full 3-phase scan on the given paths.
    ///
    /// Each path is a (directory, include_subdirs) tuple.
    pub fn scan_paths(
        &self,
        paths: &[(PathBuf, bool)],
    ) -> HashMap<FileCategory, Vec<DuplicateGroup>> {
        let mut all_results: HashMap<FileCategory, Vec<DuplicateGroup>> = HashMap::new();

        self.send(ScanProgress::Status(
            "Phase 1/3: Discovering files...".to_string(),
        ));

        let mut files_by_category: HashMap<FileCategory, Vec<FileInfo>> = HashMap::new();
        let mut total_skipped_permission: u32 = 0;
        let mut total_skipped_other: u32 = 0;

        for (path, include_subdirs) in paths {
            if self.control.is_stopped() {
                return all_results;
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

        // Phase 2: Analysis (per category)
        let categories: Vec<FileCategory> = files_by_category.keys().copied().collect();

        for category in &categories {
            if self.control.is_stopped() {
                return all_results;
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

            let groups = self.find_duplicates(files, detector.as_ref());

            if !groups.is_empty() {
                self.send(ScanProgress::PhaseCompleted {
                    category: *category,
                    groups: groups.clone(),
                });
                all_results.insert(*category, groups);
            }
        }

        self.send(ScanProgress::Completed);
        all_results
    }

    /// Phase 2: Compute signatures and find duplicate groups for a single category.
    fn find_duplicates(&self, files: &[FileInfo], detector: &dyn Detector) -> Vec<DuplicateGroup> {
        // 2a: Compute signatures in parallel using rayon
        let control = self.control.clone();
        let file_count = files.len() as u32;
        let completed = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let signatures: Vec<(PathBuf, Option<String>)> = files
            .par_iter()
            .filter(|_| !control.is_stopped())
            .map(|file| {
                // Check pause/stop
                if !control.wait_if_paused() {
                    return (file.path.clone(), None);
                }

                let sig = detector.compute_signature(&file.path).ok().flatten();

                // Progress reporting with atomic counter (monotonically increasing)
                let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                if done.is_multiple_of(10) || done == file_count {
                    let _ = self.progress_tx.send(ScanProgress::Progress {
                        current: done,
                        total: file_count,
                        message: format!("Computing signatures: {}/{}", done, file_count),
                    });
                }

                (file.path.clone(), sig)
            })
            .collect();

        if self.control.is_stopped() {
            return Vec::new();
        }

        // Build signature map (only successful signatures)
        let file_signatures: HashMap<PathBuf, String> = signatures
            .into_iter()
            .filter_map(|(path, sig)| sig.map(|s| (path, s)))
            .collect();

        self.send(ScanProgress::Status(format!(
            "Phase 2/3: Computed {} signatures, finding duplicates...",
            file_signatures.len()
        )));

        // Find duplicate groups.
        // Categories that support fuzzy matching use ONLY fuzzy matching
        // (which naturally catches exact matches at similarity 1.0).
        // Archives use exact matching only (SHA256 comparison).
        let category = detector.category();
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
                    8, // prefix length
                    Some(&|done, total| {
                        let _ = progress_tx.send(ScanProgress::Progress {
                            current: done as u32,
                            total: total as u32,
                            message: format!("Comparing signatures: {}/{}", done, total),
                        });
                    }),
                )
            }
            FileCategory::Archive => grouping::find_exact_groups(&file_signatures),
        };

        all_groups
    }
}
