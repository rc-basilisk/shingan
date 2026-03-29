use crate::cache::BoundedCache;
use crate::detector::Detector;
use crate::error::{Error, Result};
use crate::file_info::FileCategory;
use parking_lot::Mutex;
use std::path::Path;
use vid_dup_finder_lib::{VideoHash, VideoHashBuilder};

pub struct VideoDetector {
    threshold: f64,
    builder: VideoHashBuilder,
    cache: Mutex<BoundedCache<String, VideoHash>>,
    parse_cache: Mutex<BoundedCache<String, VideoHash>>,
}

impl VideoDetector {
    pub fn new(threshold: f64) -> Self {
        let options = vid_dup_finder_lib::CreationOptions {
            skip_forward_amount: 3.0,
            duration: 20.0,
            ..Default::default()
        };

        Self {
            threshold,
            builder: VideoHashBuilder::from_options(options),
            cache: Mutex::new(BoundedCache::new(1000)),
            parse_cache: Mutex::new(BoundedCache::new(2000)),
        }
    }

    fn get_hash(&self, path: &Path) -> Result<VideoHash> {
        let key = path.to_string_lossy().to_string();

        {
            let mut cache = self.cache.lock();
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

        {
            let mut cache = self.cache.lock();
            cache.put(key, hash.clone());
        }

        Ok(hash)
    }

    fn get_or_parse_video(&self, sig: &str) -> Option<VideoHash> {
        let key = sig.to_string();
        {
            let mut cache = self.parse_cache.lock();
            if let Some(cached) = cache.get(&key) {
                return Some(cached.clone());
            }
        }

        let hash: VideoHash = serde_json::from_str(sig).ok()?;

        {
            let mut cache = self.parse_cache.lock();
            cache.put(key, hash.clone());
        }

        Some(hash)
    }
}

impl Detector for VideoDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        match self.get_hash(path) {
            Ok(hash) => {
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
        let hash1 = match self.get_or_parse_video(sig1) {
            Some(h) => h,
            None => return 0.0,
        };
        let hash2 = match self.get_or_parse_video(sig2) {
            Some(h) => h,
            None => return 0.0,
        };

        let distance = hash1.hamming_distance(&hash2);

        let max_distance = 64.0_f64;
        1.0 - (distance as f64 / max_distance).min(1.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_same_signature() {
        let det = VideoDetector::new(0.9);
        let zeros: Vec<usize> = vec![0; 16];
        let sig = format!(
            "{{\"hash\":{:?},\"src_path\":\"/tmp/test.mp4\",\"duration\":0}}",
            zeros
        );
        let sim = det.compare_signatures(&sig, &sig);
        assert!((sim - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compare_invalid_signature() {
        let det = VideoDetector::new(0.9);
        let sim = det.compare_signatures("not json", "also not json");
        assert!((sim - 0.0).abs() < f64::EPSILON);
    }
}
