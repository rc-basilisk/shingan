use crate::error::Result;
use crate::file_info::FileCategory;
use std::path::Path;

pub mod archive;
#[cfg(feature = "code-detect")]
pub mod code;
#[cfg(feature = "document-detect")]
pub mod document;
#[cfg(feature = "image-detect")]
pub mod image;
#[cfg(feature = "video-detect")]
pub mod video;

/// Trait implemented by all file-type-specific duplicate detectors.
///
/// Each detector can compute signatures for files of its category and
/// compare them for similarity.
pub trait Detector: Send + Sync {
    /// Compute a signature/fingerprint for the file.
    /// Returns `Ok(None)` if the file cannot be processed (corrupt, empty, etc.).
    fn compute_signature(&self, path: &Path) -> Result<Option<String>>;

    /// Compare two files directly and return a similarity score in `0.0..=1.0`.
    fn compare_files(&self, file1: &Path, file2: &Path) -> Result<f64>;

    /// Compare two pre-computed signatures. Returns similarity in `0.0..=1.0`.
    /// Default implementation: exact match (1.0) or no match (0.0).
    fn compare_signatures(&self, sig1: &str, sig2: &str) -> f64 {
        if sig1 == sig2 {
            1.0
        } else {
            0.0
        }
    }

    /// The file category this detector handles.
    fn category(&self) -> FileCategory;

    /// The similarity threshold for this detector.
    fn threshold(&self) -> f64;
}

/// Compute SHA-256 hash of a file, streaming in chunks.
pub fn file_sha256(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| crate::error::Error::io(path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|e| crate::error::Error::io(path, e))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}
