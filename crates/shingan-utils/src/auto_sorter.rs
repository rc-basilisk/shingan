use shingan_core::file_info::ExtensionMap;
use std::path::{Path, PathBuf};

/// Sorts files from source directories into category-based subdirectories.
pub struct AutoSorter {
    source_paths: Vec<PathBuf>,
    destination: PathBuf,
    extension_map: ExtensionMap,
}

/// Statistics returned after sorting completes.
#[derive(Debug, Clone, Default)]
pub struct SortStats {
    pub total: u64,
    pub moved: u64,
    pub failed: u64,
    pub skipped: u64,
}

impl AutoSorter {
    pub fn new(source_paths: Vec<PathBuf>, destination: PathBuf) -> Self {
        Self {
            source_paths,
            destination,
            extension_map: ExtensionMap::new(),
        }
    }

    /// Sort files into category directories.
    pub fn sort_files(
        &self,
        progress: Option<&dyn Fn(u64, u64, &str)>,
        status: Option<&dyn Fn(&str)>,
    ) -> SortStats {
        let mut stats = SortStats::default();

        // Collect all files first
        let mut all_files: Vec<PathBuf> = Vec::new();
        for source in &self.source_paths {
            if let Ok(entries) = walkdir::WalkDir::new(source).into_iter().collect::<Result<Vec<_>, _>>() {
                for entry in entries {
                    if entry.file_type().is_file() {
                        all_files.push(entry.into_path());
                    }
                }
            }
        }

        stats.total = all_files.len() as u64;

        if let Some(cb) = status {
            cb(&format!("Found {} files to sort", stats.total));
        }

        for (i, file_path) in all_files.iter().enumerate() {
            if let Some(cb) = progress {
                cb(i as u64 + 1, stats.total, &file_path.to_string_lossy());
            }

            let ext = match file_path.extension().and_then(|e| e.to_str()) {
                Some(e) => e.to_lowercase(),
                None => {
                    stats.skipped += 1;
                    continue;
                }
            };

            let category_name = self
                .extension_map
                .get(&ext)
                .map(|c| format!("{}s", c.label()))
                .unwrap_or_else(|| "others".to_string());

            let dest_dir = self.destination.join(&category_name);
            if std::fs::create_dir_all(&dest_dir).is_err() {
                stats.failed += 1;
                continue;
            }

            let file_name = file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let dest_path = resolve_conflict(&dest_dir, &file_name);

            match std::fs::rename(file_path, &dest_path) {
                Ok(_) => stats.moved += 1,
                Err(_) => {
                    // Try copy + delete as fallback (cross-device move)
                    match std::fs::copy(file_path, &dest_path) {
                        Ok(_) => {
                            let _ = std::fs::remove_file(file_path);
                            stats.moved += 1;
                        }
                        Err(_) => stats.failed += 1,
                    }
                }
            }
        }

        if let Some(cb) = status {
            cb(&format!(
                "Sorting complete: {} moved, {} failed, {} skipped",
                stats.moved, stats.failed, stats.skipped
            ));
        }

        stats
    }
}

/// Resolve filename conflicts by appending _1, _2, etc.
fn resolve_conflict(dir: &Path, filename: &str) -> PathBuf {
    let dest = dir.join(filename);
    if !dest.exists() {
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
        let new_name = format!("{}_{}{}", stem, counter, ext);
        let new_path = dir.join(&new_name);
        if !new_path.exists() {
            return new_path;
        }
        counter += 1;
    }
}
