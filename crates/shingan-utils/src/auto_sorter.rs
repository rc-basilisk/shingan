use serde::{Deserialize, Serialize};
use shingan_core::file_info::{ExtensionMap, FileCategory, FileInfo, EXCLUDED_DIRS};
use shingan_ml::{CloudCategorizer, PipelineConfig, TieredPipeline};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Callback signature for reporting sort progress (current, total, file path).
type ProgressCallback<'a> = Option<&'a dyn Fn(u64, u64, &str)>;

/// Sorts files from source directories into category-based subdirectories.
///
/// When `use_ml` is true, images are further sub-categorized via the tiered
/// ML pipeline (heuristics -> structure -> local CLIP -> optional cloud).
///
/// Safety rules:
/// - hidden files/folders and tool directories (`.git`, `node_modules`, ...)
///   are never touched;
/// - files that are already inside the destination are skipped, so running a
///   sort twice is harmless;
/// - every real (non-dry-run) sort writes an undo journal into the destination
///   (see [`undo_sort`]).
pub struct AutoSorter {
    source_paths: Vec<PathBuf>,
    destination: PathBuf,
    extension_map: ExtensionMap,
    use_ml: bool,
    dry_run: bool,
    include_unknown: bool,
    pipeline_config: PipelineConfig,
    cloud: Option<Arc<dyn CloudCategorizer>>,
    cancel: Option<Arc<AtomicBool>>,
}

/// Statistics returned after sorting completes.
#[derive(Debug, Clone, Default)]
pub struct SortStats {
    pub total: u64,
    pub moved: u64,
    pub failed: u64,
    pub skipped: u64,
    /// Every move performed (or planned, in dry-run mode): `(from, to)`.
    pub moves: Vec<(PathBuf, PathBuf)>,
    /// Human-readable error per failed file.
    pub errors: Vec<String>,
    /// Undo journal written for this run (not set for dry runs).
    pub journal: Option<PathBuf>,
    /// True if the run was cancelled before finishing.
    pub cancelled: bool,
}

/// One entry of an undo journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Prefix of undo journal files written into the destination folder.
pub const JOURNAL_PREFIX: &str = ".shingan-sort-";

impl AutoSorter {
    pub fn new(source_paths: Vec<PathBuf>, destination: PathBuf) -> Self {
        Self {
            source_paths,
            destination,
            extension_map: ExtensionMap::new(),
            use_ml: false,
            dry_run: false,
            include_unknown: true,
            pipeline_config: PipelineConfig::default(),
            cloud: None,
            cancel: None,
        }
    }

    pub fn with_ml(mut self, enable: bool) -> Self {
        self.use_ml = enable;
        self
    }

    /// Enable dry-run mode: compute destinations but don't actually move files.
    pub fn with_dry_run(mut self, enable: bool) -> Self {
        self.dry_run = enable;
        self
    }

    /// Whether files with unrecognized extensions go to `others/` (default) or stay put.
    pub fn with_include_unknown(mut self, enable: bool) -> Self {
        self.include_unknown = enable;
        self
    }

    /// Set a custom pipeline configuration (thresholds, model dir).
    pub fn with_pipeline_config(mut self, config: PipelineConfig) -> Self {
        self.pipeline_config = config;
        self
    }

    /// Attach a cloud categorizer used for images the local tiers are unsure about.
    pub fn with_cloud(mut self, cloud: Option<Arc<dyn CloudCategorizer>>) -> Self {
        self.cloud = cloud;
        self
    }

    /// A flag that, when set to `true`, stops the sort after the current file.
    pub fn with_cancel_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = Some(flag);
        self
    }

    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }

    /// Collect files to sort, skipping hidden/tool directories and the destination.
    fn collect_files(&self, dest_canon: &Path, stats: &mut SortStats) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for source in &self.source_paths {
            let walker = walkdir::WalkDir::new(source)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    if e.depth() == 0 {
                        return true;
                    }
                    let name = e.file_name().to_string_lossy();
                    if name.starts_with('.') {
                        return false;
                    }
                    if e.file_type().is_dir() {
                        if EXCLUDED_DIRS.contains(&name.as_ref()) {
                            return false;
                        }
                        // Never descend into the destination (it may live inside a source).
                        if std::fs::canonicalize(e.path()).is_ok_and(|p| p == dest_canon) {
                            return false;
                        }
                    }
                    true
                });
            for entry in walker {
                match entry {
                    Ok(e) if e.file_type().is_file() => files.push(e.into_path()),
                    Ok(_) => {}
                    Err(e) => {
                        stats.skipped += 1;
                        stats.errors.push(format!("Could not read: {e}"));
                    }
                }
            }
        }
        files.sort();
        files.dedup();
        files
    }

    /// Sort files into category directories.
    pub fn sort_files(
        &self,
        progress: ProgressCallback<'_>,
        status: Option<&dyn Fn(&str)>,
    ) -> SortStats {
        let mut stats = SortStats::default();

        if !self.dry_run {
            if let Err(e) = std::fs::create_dir_all(&self.destination) {
                stats.errors.push(format!(
                    "Cannot create destination {}: {e}",
                    self.destination.display()
                ));
                return stats;
            }
        }
        let dest_canon =
            std::fs::canonicalize(&self.destination).unwrap_or_else(|_| self.destination.clone());

        let all_files = self.collect_files(&dest_canon, &mut stats);
        stats.total = all_files.len() as u64;

        if let Some(cb) = status {
            cb(&format!("Found {} files to sort", stats.total));
        }

        let mut pipeline = if self.use_ml {
            let p =
                TieredPipeline::new(self.pipeline_config.clone()).with_cloud(self.cloud.clone());
            if let Some(cb) = status {
                if let Some(err) = p.onnx_error() {
                    cb(&format!("Local CLIP model unavailable: {err}"));
                } else if !p.has_clip() {
                    cb("Local CLIP model not installed; using heuristics only (download it in Settings)");
                }
            }
            Some(p)
        } else {
            None
        };

        let mut reserved: HashSet<PathBuf> = HashSet::new();
        let mut journal: Vec<JournalEntry> = Vec::new();

        for (i, file_path) in all_files.iter().enumerate() {
            if self.is_cancelled() {
                stats.cancelled = true;
                break;
            }
            if let Some(cb) = progress {
                cb(i as u64 + 1, stats.total, &file_path.to_string_lossy());
            }

            let category = file_path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(|e| self.extension_map.get(e));
            if category.is_none() && !self.include_unknown {
                stats.skipped += 1;
                continue;
            }
            let category_name = category
                .map(|c| format!("{}s", c.label()))
                .unwrap_or_else(|| "others".to_string());

            let sub_folder = match (&mut pipeline, category) {
                (Some(p), Some(FileCategory::Image)) => classify_image(file_path, p),
                _ => None,
            };

            let dest_dir = match &sub_folder {
                Some(sub) => self.destination.join(&category_name).join(sub),
                None => self.destination.join(&category_name),
            };

            if !self.dry_run {
                if let Err(e) = std::fs::create_dir_all(&dest_dir) {
                    stats.failed += 1;
                    stats.errors.push(format!(
                        "{}: cannot create {}: {e}",
                        file_path.display(),
                        dest_dir.display()
                    ));
                    continue;
                }
            }

            let file_name = file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let dest_path = resolve_conflict(&dest_dir, &file_name, &reserved);
            reserved.insert(dest_path.clone());

            if self.dry_run {
                if let Some(cb) = status {
                    cb(&format!(
                        "  {} -> {}",
                        file_path.display(),
                        dest_path.display()
                    ));
                }
                stats.moved += 1;
                stats.moves.push((file_path.clone(), dest_path));
                continue;
            }

            match move_file(file_path, &dest_path) {
                Ok(()) => {
                    stats.moved += 1;
                    journal.push(JournalEntry {
                        from: file_path.clone(),
                        to: dest_path.clone(),
                    });
                    stats.moves.push((file_path.clone(), dest_path));
                }
                Err(e) => {
                    stats.failed += 1;
                    stats.errors.push(format!("{}: {e}", file_path.display()));
                }
            }
        }

        if !journal.is_empty() {
            match write_journal(&self.destination, &journal) {
                Ok(p) => stats.journal = Some(p),
                Err(e) => stats
                    .errors
                    .push(format!("Could not write undo journal: {e}")),
            }
        }

        if let (Some(cb), Some(p)) = (status, &pipeline) {
            let (requests, errors) = p.cloud_usage();
            if requests > 0 {
                cb(&format!("Cloud requests: {requests} ({errors} failed)"));
            }
        }

        if let Some(cb) = status {
            cb(&format!(
                "Sorting {}: {} {}, {} failed, {} skipped",
                if stats.cancelled {
                    "cancelled"
                } else {
                    "complete"
                },
                stats.moved,
                if self.dry_run {
                    "would be moved"
                } else {
                    "moved"
                },
                stats.failed,
                stats.skipped
            ));
        }

        stats
    }
}

fn classify_image(path: &Path, pipeline: &mut TieredPipeline) -> Option<String> {
    let mut info = FileInfo::from_path(path, FileCategory::Image).ok()?;
    info.enrich_metadata();
    let result = pipeline.classify_local(path, &info);
    Some(result.category.label().to_string())
}

/// Move a file, falling back to copy + delete across filesystems.
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| format!("copy failed: {e}"))?;
    // Keep the original modification time on the copy.
    if let Ok(mtime) = std::fs::metadata(from).and_then(|m| m.modified()) {
        if let Ok(f) = std::fs::File::options().write(true).open(to) {
            let _ = f.set_modified(mtime);
        }
    }
    if let Err(e) = std::fs::remove_file(from) {
        // Don't leave two copies behind.
        let _ = std::fs::remove_file(to);
        return Err(format!("could not remove original after copying: {e}"));
    }
    Ok(())
}

fn write_journal(dest: &Path, entries: &[JournalEntry]) -> std::io::Result<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dest.join(format!("{JOURNAL_PREFIX}{secs}.json"));
    let json = serde_json::to_string_pretty(entries).map_err(std::io::Error::other)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

/// The most recent undo journal in `dest`, if any.
pub fn latest_journal(dest: &Path) -> Option<PathBuf> {
    let mut journals: Vec<PathBuf> = std::fs::read_dir(dest)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(JOURNAL_PREFIX) && n.ends_with(".json"))
        })
        .collect();
    journals.sort();
    journals.pop()
}

/// Result of undoing a sort.
#[derive(Debug, Default, Clone)]
pub struct UndoStats {
    pub restored: u64,
    pub failed: u64,
    pub errors: Vec<String>,
}

/// Move every file listed in an undo journal back to where it came from.
///
/// Files that were moved or deleted since, or whose original location is now
/// occupied, are reported and left alone. Empty category folders are removed.
/// The journal is deleted when everything was restored.
pub fn undo_sort(journal_path: &Path) -> Result<UndoStats, String> {
    let content = std::fs::read_to_string(journal_path).map_err(|e| e.to_string())?;
    let entries: Vec<JournalEntry> =
        serde_json::from_str(&content).map_err(|e| format!("invalid journal: {e}"))?;
    let mut stats = UndoStats::default();
    let mut touched_dirs: HashSet<PathBuf> = HashSet::new();

    for entry in entries.iter().rev() {
        if !entry.to.is_file() {
            stats.failed += 1;
            stats
                .errors
                .push(format!("{} no longer exists", entry.to.display()));
            continue;
        }
        if entry.from.exists() {
            stats.failed += 1;
            stats.errors.push(format!(
                "{} already exists; not overwriting",
                entry.from.display()
            ));
            continue;
        }
        if let Some(parent) = entry.from.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match move_file(&entry.to, &entry.from) {
            Ok(()) => {
                stats.restored += 1;
                if let Some(p) = entry.to.parent() {
                    touched_dirs.insert(p.to_path_buf());
                    if let Some(pp) = p.parent() {
                        touched_dirs.insert(pp.to_path_buf());
                    }
                }
            }
            Err(e) => {
                stats.failed += 1;
                stats.errors.push(format!("{}: {e}", entry.to.display()));
            }
        }
    }

    // Remove now-empty sub-folders (deepest first); remove_dir fails on non-empty ones.
    let mut dirs: Vec<PathBuf> = touched_dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    let dest = journal_path.parent().map(Path::to_path_buf);
    for d in dirs {
        if Some(&d) != dest.as_ref() {
            let _ = std::fs::remove_dir(&d);
        }
    }

    if stats.failed == 0 {
        let _ = std::fs::remove_file(journal_path);
    }
    Ok(stats)
}

/// Resolve filename conflicts by appending _1, _2, etc. Considers both files on
/// disk and destinations already claimed during this run.
fn resolve_conflict(dir: &Path, filename: &str, reserved: &HashSet<PathBuf>) -> PathBuf {
    let taken = |p: &Path| p.exists() || reserved.contains(p);
    let dest = dir.join(filename);
    if !taken(&dest) {
        return dest;
    }

    let stem = Path::new(filename)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let ext = Path::new(filename)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let mut counter = 1u32;
    loop {
        let new_path = dir.join(format!("{}_{}{}", stem, counter, ext));
        if !taken(&new_path) {
            return new_path;
        }
        counter += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = src.join("sorted"); // destination inside the source
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join("a.txt"), "a").unwrap();
        std::fs::write(src.join("sub").join("a.txt"), "other a").unwrap();
        std::fs::write(src.join("pic.png"), "png").unwrap();
        std::fs::write(src.join(".hidden.txt"), "h").unwrap();
        std::fs::write(src.join(".git").join("config"), "git").unwrap();
        std::fs::write(src.join("blob.xyz"), "?").unwrap();
        (dir, src, dest)
    }

    #[test]
    fn sorts_and_skips_hidden_and_tool_dirs() {
        let (_d, src, dest) = setup();
        let stats = AutoSorter::new(vec![src.clone()], dest.clone()).sort_files(None, None);
        assert_eq!(stats.moved, 4, "{:?}", stats.moves);
        assert!(dest.join("documents/a.txt").is_file());
        assert!(dest.join("documents/a_1.txt").is_file());
        assert!(dest.join("images/pic.png").is_file());
        assert!(dest.join("others/blob.xyz").is_file());
        assert!(src.join(".hidden.txt").is_file());
        assert!(src.join(".git/config").is_file());
        assert!(stats.journal.is_some());

        // Running again must not touch already-sorted files.
        let again = AutoSorter::new(vec![src.clone()], dest.clone()).sort_files(None, None);
        assert_eq!(again.moved, 0);
        assert!(dest.join("documents/a.txt").is_file());
    }

    #[test]
    fn dry_run_moves_nothing_and_resolves_conflicts() {
        let (_d, src, dest) = setup();
        let stats = AutoSorter::new(vec![src.clone()], dest.clone())
            .with_dry_run(true)
            .sort_files(None, None);
        assert_eq!(stats.moved, 4);
        assert!(src.join("a.txt").is_file());
        assert!(!dest.exists());
        let targets: HashSet<_> = stats.moves.iter().map(|(_, to)| to.clone()).collect();
        assert_eq!(targets.len(), 4, "dry-run targets must be distinct");
    }

    #[test]
    fn undo_restores_everything() {
        let (_d, src, dest) = setup();
        let stats = AutoSorter::new(vec![src.clone()], dest.clone()).sort_files(None, None);
        let journal = stats.journal.unwrap();
        assert_eq!(latest_journal(&dest), Some(journal.clone()));
        let undo = undo_sort(&journal).unwrap();
        assert_eq!(undo.restored, 4);
        assert_eq!(undo.failed, 0);
        assert_eq!(
            std::fs::read_to_string(src.join("sub/a.txt")).unwrap(),
            "other a"
        );
        assert!(src.join("pic.png").is_file());
        assert!(!journal.exists());
        assert!(!dest.join("documents").exists());
    }

    #[test]
    fn unknown_files_can_be_left_in_place() {
        let (_d, src, dest) = setup();
        let stats = AutoSorter::new(vec![src.clone()], dest)
            .with_include_unknown(false)
            .sort_files(None, None);
        assert_eq!(stats.moved, 3);
        assert!(src.join("blob.xyz").is_file());
    }
}
