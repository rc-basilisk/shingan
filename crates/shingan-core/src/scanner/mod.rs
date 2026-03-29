pub mod duplicate;
pub mod grouping;

use crate::file_info::{ExtensionMap, FileCategory, FileInfo, EXCLUDED_DIRS};
use std::collections::HashSet;
use std::path::Path;

/// Scans directories for files matching selected categories.
pub struct FileScanner {
    extension_map: ExtensionMap,
    categories: HashSet<FileCategory>,
}

impl FileScanner {
    pub fn new(categories: &[FileCategory]) -> Self {
        Self {
            extension_map: ExtensionMap::new(),
            categories: categories.iter().copied().collect(),
        }
    }

    /// Scan a directory and return all matching files.
    pub fn scan_directory(
        &self,
        root: &Path,
        include_subdirs: bool,
        progress: Option<&dyn Fn(&Path, usize)>,
    ) -> Vec<FileInfo> {
        let mut files = Vec::new();

        let walker = if include_subdirs {
            walkdir::WalkDir::new(root)
                .follow_links(false)
                .into_iter()
        } else {
            walkdir::WalkDir::new(root)
                .max_depth(1)
                .follow_links(false)
                .into_iter()
        };

        for entry in walker.filter_entry(|e| !is_excluded_dir(e)) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
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
                    if let Some(cb) = progress {
                        cb(path, files.len() + 1);
                    }
                    files.push(info);
                }
                Err(_) => continue,
            }
        }

        files
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
