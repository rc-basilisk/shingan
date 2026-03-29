use crate::cache::BoundedCache;
use crate::detector::Detector;
use crate::error::{Error, Result};
use crate::file_info::FileCategory;
use std::path::Path;
use std::sync::Mutex;
use vid_dup_finder_lib::{VideoHash, VideoHashBuilder};

/// Video duplicate detector using perceptual hashing via vid_dup_finder_lib.
///
/// Samples evenly-spaced frames from the first N seconds of each video,
/// computes a 3D DCT-based perceptual hash (spatial + temporal), and
/// compares via hamming distance. Requires ffmpeg/ffprobe on PATH.
pub struct VideoDetector {
    threshold: f64,
    builder: VideoHashBuilder,
    cache: Mutex<BoundedCache<String, VideoHash>>,
}

impl VideoDetector {
    pub fn new(threshold: f64) -> Self {
        let options = vid_dup_finder_lib::CreationOptions {
            skip_forward_amount: 3.0, // skip 3s of intros
            duration: 20.0,           // sample first 20s
            ..Default::default()
        };

        Self {
            threshold,
            builder: VideoHashBuilder::from_options(options),
            cache: Mutex::new(BoundedCache::new(1000)),
        }
    }

    fn get_hash(&self, path: &Path) -> Result<VideoHash> {
        let key = path.to_string_lossy().to_string();

        if let Ok(mut cache) = self.cache.lock() {
            if let Some(hash) = cache.get(&key) {
                return Ok(hash.clone());
            }
        }

        let hash = self
            .builder
            .hash(path.to_path_buf())
            .map_err(|e| Error::Signature {
                path: path.to_path_buf(),
                reason: format!("{}", e),
            })?;

        if let Ok(mut cache) = self.cache.lock() {
            cache.put(key, hash.clone());
        }

        Ok(hash)
    }
}

impl Detector for VideoDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        match self.get_hash(path) {
            Ok(hash) => {
                // Serialize the hash to JSON for storage/comparison
                let sig = serde_json::to_string(&hash).map_err(|e| Error::Signature {
                    path: path.to_path_buf(),
                    reason: format!("serialize: {}", e),
                })?;
                Ok(Some(sig))
            }
            Err(_) => Ok(None),
        }
    }

    fn compare_signatures(&self, sig1: &str, sig2: &str) -> f64 {
        let hash1: VideoHash = match serde_json::from_str(sig1) {
            Ok(h) => h,
            Err(_) => return 0.0,
        };
        let hash2: VideoHash = match serde_json::from_str(sig2) {
            Ok(h) => h,
            Err(_) => return 0.0,
        };

        let distance = hash1.hamming_distance(&hash2);

        // Normalize: vid_dup_finder_lib uses 0.3 as default tolerance.
        // The hash is a bitvec; max distance depends on hash size.
        // We use a heuristic: distance 0 = 1.0 similarity,
        // distance >= 64 = 0.0 similarity (most hashes are ~64 bits).
        let max_distance = 64.0_f64;
        let similarity = 1.0 - (distance as f64 / max_distance).min(1.0);
        similarity
    }

    fn compare_files(&self, file1: &Path, file2: &Path) -> Result<f64> {
        let hash1 = self.get_hash(file1)?;
        let hash2 = self.get_hash(file2)?;
        let distance = hash1.hamming_distance(&hash2);
        let max_distance = 64.0_f64;
        Ok(1.0 - (distance as f64 / max_distance).min(1.0))
    }

    fn category(&self) -> FileCategory {
        FileCategory::Video
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }
}
