use crate::detector::{file_sha256, Detector};
use crate::error::Result;
use crate::file_info::FileCategory;
use std::path::Path;

/// Archive duplicate detector using SHA-256 exact matching.
///
/// Archives are compared byte-for-byte via their hash — no fuzzy matching.
pub struct ArchiveDetector {
    threshold: f64,
}

impl ArchiveDetector {
    pub fn new(_threshold: f64) -> Self {
        // Archives always use exact matching regardless of configured threshold
        Self { threshold: 1.0 }
    }
}

impl Detector for ArchiveDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        match file_sha256(path) {
            Ok(hash) => Ok(Some(hash)),
            Err(_) => Ok(None),
        }
    }

    fn compare_files(&self, file1: &Path, file2: &Path) -> Result<f64> {
        let sig1 = self.compute_signature(file1)?;
        let sig2 = self.compute_signature(file2)?;
        match (sig1, sig2) {
            (Some(s1), Some(s2)) => Ok(self.compare_signatures(&s1, &s2)),
            _ => Ok(0.0),
        }
    }

    fn category(&self) -> FileCategory {
        FileCategory::Archive
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }
}
