pub mod duplicate;
pub mod grouping;

use crate::file_info::{ExtensionMap, FileCategory, FileInfo, EXCLUDED_DIRS};
use std::collections::HashSet;
use std::io;
use std::path::Path;

/// Callback signature for reporting scan progress (path, file count so far).
type ScanProgressCallback<'a> = Option<&'a dyn Fn(&Path, usize)>;

pub struct ScanResult {
    pub files: Vec<FileInfo>,
    pub skipped_permission: u32,
    pub skipped_other: u32,
}

/// Scans directories for files matching selected categories.
pub struct FileScanner {
    extension_map: ExtensionMap,
    categories: HashSet<FileCategory>,
    min_size: u64,
    max_size: Option<u64>,
}

impl FileScanner {
    pub fn new(categories: &[FileCategory]) -> Self {
        Self {
            extension_map: ExtensionMap::new(),
            categories: categories.iter().copied().collect(),
            min_size: 0,
            max_size: None,
        }
    }

    pub fn with_size_limits(mut self, min: u64, max: Option<u64>) -> Self {
        self.min_size = min;
        self.max_size = max;
        self
    }

    /// Scan a directory and return all matching files.
    pub fn scan_directory(
        &self,
        root: &Path,
        include_subdirs: bool,
        progress: ScanProgressCallback<'_>,
    ) -> ScanResult {
        let mut files = Vec::new();
        let mut skipped_permission: u32 = 0;
        let mut skipped_other: u32 = 0;

        let walker = if include_subdirs {
            walkdir::WalkDir::new(root).follow_links(false).into_iter()
        } else {
            walkdir::WalkDir::new(root)
                .max_depth(1)
                .follow_links(false)
                .into_iter()
        };

        for entry in walker.filter_entry(|e| !is_excluded_dir(e)) {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    if let Some(io_err) = e.io_error() {
                        if io_err.kind() == io::ErrorKind::PermissionDenied {
                            skipped_permission += 1;
                        } else {
                            skipped_other += 1;
                        }
                    } else {
                        skipped_other += 1;
                    }
                    continue;
                }
            };

            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();
            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(e) => e.to_lowercase(),
                None => continue,
            };

            let category = match self.extension_map.get(&ext) {
                Some(cat) if self.categories.contains(&cat) => cat,
                _ => continue,
            };

            match FileInfo::from_path(path, category) {
                Ok(info) => {
                    if info.size < self.min_size {
                        skipped_other += 1;
                        continue;
                    }
                    if let Some(max) = self.max_size {
                        if info.size > max {
                            skipped_other += 1;
                            continue;
                        }
                    }
                    if let Some(cb) = progress {
                        cb(path, files.len() + 1);
                    }
                    files.push(info);
                }
                Err(_) => {
                    skipped_other += 1;
                    continue;
                }
            }
        }

        ScanResult {
            files,
            skipped_permission,
            skipped_other,
        }
    }

    /// Get the category for a file extension.
    pub fn get_category(&self, extension: &str) -> Option<FileCategory> {
        self.extension_map.get(extension)
    }
}

fn is_excluded_dir(entry: &walkdir::DirEntry) -> bool {
    if entry.file_type().is_dir() {
        if let Some(name) = entry.file_name().to_str() {
            return EXCLUDED_DIRS.contains(&name);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_scan_finds_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("test.txt"), "hello world").unwrap();
        fs::write(dir.path().join("test.png"), vec![0u8; 2048]).unwrap();

        let scanner = FileScanner::new(&[FileCategory::Image]);
        let result = scanner.scan_directory(dir.path(), false, None);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].extension, "png");
    }

    #[test]
    fn test_scan_skips_excluded_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let nm = dir.path().join("node_modules");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("test.png"), vec![0u8; 2048]).unwrap();
        fs::write(dir.path().join("good.png"), vec![0u8; 2048]).unwrap();

        let scanner = FileScanner::new(&[FileCategory::Image]);
        let result = scanner.scan_directory(dir.path(), true, None);
        assert_eq!(result.files.len(), 1);
        assert!(result.files[0].path.ends_with("good.png"));
    }

    #[test]
    fn test_scan_respects_size_limits() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("tiny.png"), [0u8; 16]).unwrap();
        fs::write(dir.path().join("big.png"), [0u8; 2048]).unwrap();

        let scanner = FileScanner::new(&[FileCategory::Image]).with_size_limits(1024, None);
        let result = scanner.scan_directory(dir.path(), false, None);
        assert_eq!(result.files.len(), 1);
        assert!(result.files[0].path.ends_with("big.png"));
        assert!(result.skipped_other >= 1);
    }

    #[test]
    fn test_scan_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let scanner = FileScanner::new(&[FileCategory::Image]);
        let result = scanner.scan_directory(dir.path(), false, None);
        assert!(result.files.is_empty());
        assert_eq!(result.skipped_permission, 0);
        assert_eq!(result.skipped_other, 0);
    }
}
