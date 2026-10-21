use crate::detector::{file_sha256, Detector};
use crate::error::Result;
use crate::file_info::FileCategory;
use std::path::Path;

/// Archive duplicate detector using SHA-256 exact matching.
///
/// Archives are compared byte-for-byte via their hash — no fuzzy matching.
/// Because only identical files can match, the scanner skips hashing any
/// archive whose size is unique.
pub struct ArchiveDetector;

impl ArchiveDetector {
    /// The threshold is ignored: archives always use exact matching.
    pub fn new(_threshold: f64) -> Self {
        Self
    }
}

impl Detector for ArchiveDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        Ok(file_sha256(path).ok())
    }

    fn exact_only(&self) -> bool {
        true
    }

    fn category(&self) -> FileCategory {
        FileCategory::Archive
    }

    fn threshold(&self) -> f64 {
        1.0
    }
}
