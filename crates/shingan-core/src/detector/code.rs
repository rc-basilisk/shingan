use crate::cache::BoundedCache;
use crate::detector::Detector;
use crate::error::Result;
use crate::file_info::FileCategory;
use parking_lot::Mutex;
use std::path::Path;

/// Code duplicate detector using normalization and fuzzy string matching.
///
/// Normalizes code by stripping comments and whitespace, then uses
/// `token_set_ratio` for fuzzy comparison.
pub struct CodeDetector {
    threshold: f64,
    cache: Mutex<BoundedCache<String, String>>,
}

impl CodeDetector {
    pub fn new(threshold: f64) -> Self {
        Self {
            threshold,
            cache: Mutex::new(BoundedCache::new(1000)),
        }
    }

    /// Read and normalize code: strip comments, blank lines, excess whitespace.
    fn normalize_code(&self, path: &Path) -> Option<String> {
        let key = path.to_string_lossy().to_string();

        {
            let mut cache = self.cache.lock();
            if let Some(text) = cache.get(&key) {
                return Some(text.clone());
            }
        }

        let content = Self::read_with_fallback_encoding(path)?;

        let normalized: Vec<&str> = content
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .filter(|line| {
                !line.starts_with('#') && !line.starts_with("//") && !line.starts_with("/*")
            })
            .collect();

        if normalized.is_empty() {
            return None;
        }

        let result = normalized.join(" ");

        {
            let mut cache = self.cache.lock();
            cache.put(key, result.clone());
        }

        Some(result)
    }

    fn read_with_fallback_encoding(path: &Path) -> Option<String> {
        // Try UTF-8 first
        if let Ok(content) = std::fs::read_to_string(path) {
            return Some(content);
        }
        // Fall back to reading as bytes and lossy conversion
        if let Ok(bytes) = std::fs::read(path) {
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
        None
    }
}

impl Detector for CodeDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        match self.normalize_code(path) {
            Some(_) => {
                // Use SHA-256 of normalized code as signature
                // (fuzzy matching happens via compare_files through the LSH path)
                let normalized = self.normalize_code(path).unwrap();
                use sha2::{Digest, Sha256};
                let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
                Ok(Some(hash))
            }
            None => Ok(None),
        }
    }

    fn compare_files(&self, file1: &Path, file2: &Path) -> Result<f64> {
        let text1 = self.normalize_code(file1);
        let text2 = self.normalize_code(file2);

        match (text1, text2) {
            (Some(t1), Some(t2)) => Ok(strsim::sorensen_dice(&t1, &t2)),
            _ => Ok(0.0),
        }
    }

    fn category(&self) -> FileCategory {
        FileCategory::Code
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_missing_file() {
        let det = CodeDetector::new(0.9);
        let result = det.normalize_code(Path::new("/nonexistent/path/file.py"));
        assert!(result.is_none());
    }
}
