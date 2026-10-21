use crate::detector::{Detector, PreparedSignatures};
use crate::error::{Error, Result};
use crate::file_info::FileCategory;
use std::path::Path;
use vid_dup_finder_lib::{VideoHash, VideoHashBuilder};

/// Number of bits in a `vid_dup_finder_lib` 0.4 hash (a 10x10x10 DCT cube).
/// The crate only exports this constant for tests, so it is mirrored here.
const HASH_BITS: f64 = 1000.0;

/// Video duplicate detector using `vid_dup_finder_lib`'s 3-D DCT fingerprint.
///
/// Similarity is `1 - hamming_distance / 1000`. For reference, the library's
/// own default search tolerance corresponds to a similarity of about 0.7, so
/// thresholds in the 0.80-0.95 range are fairly strict.
pub struct VideoDetector {
    threshold: f64,
    builder: VideoHashBuilder,
}

impl VideoDetector {
    pub fn new(threshold: f64) -> Self {
        Self::with_sampling(threshold, 3.0, 20.0)
    }

    /// Create a video detector with custom sampling parameters.
    ///
    /// - `skip_secs`: seconds to skip at the start of the video (e.g. intro).
    /// - `duration_secs`: how many seconds of the video to sample for hashing.
    pub fn with_sampling(threshold: f64, skip_secs: f64, duration_secs: f64) -> Self {
        let options = vid_dup_finder_lib::CreationOptions {
            skip_forward_amount: skip_secs.max(0.0),
            duration: duration_secs.max(1.0),
            ..Default::default()
        };

        Self {
            threshold,
            builder: VideoHashBuilder::from_options(options),
        }
    }

    fn hash_file(&self, path: &Path) -> Result<VideoHash> {
        self.builder
            .hash(path.to_path_buf())
            .map_err(|e| Error::Signature {
                path: path.to_path_buf(),
                reason: format!("{}", e),
            })
    }

    fn similarity_of(a: &VideoHash, b: &VideoHash) -> f64 {
        1.0 - (a.hamming_distance(b) as f64 / HASH_BITS).min(1.0)
    }
}

impl Detector for VideoDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        match self.hash_file(path) {
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
        match (
            serde_json::from_str::<VideoHash>(sig1),
            serde_json::from_str::<VideoHash>(sig2),
        ) {
            (Ok(a), Ok(b)) => Self::similarity_of(&a, &b),
            _ => 0.0,
        }
    }

    /// Video hashes have no cheap bucketing scheme here; compare all pairs.
    /// Comparisons are fast (a few XOR/popcounts) once signatures are parsed.
    fn candidate_keys(&self, _sig: &str, _threshold: f64) -> Vec<u64> {
        vec![0]
    }

    fn prepare<'a>(&'a self, sigs: &[&'a str]) -> Box<dyn PreparedSignatures + 'a> {
        Box::new(PreparedVideos {
            parsed: sigs
                .iter()
                .map(|s| serde_json::from_str::<VideoHash>(s).ok())
                .collect(),
        })
    }

    fn category(&self) -> FileCategory {
        FileCategory::Video
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }
}

struct PreparedVideos {
    parsed: Vec<Option<VideoHash>>,
}

impl PreparedSignatures for PreparedVideos {
    fn similarity(&self, a: usize, b: usize) -> f64 {
        match (&self.parsed[a], &self.parsed[b]) {
            (Some(x), Some(y)) => VideoDetector::similarity_of(x, y),
            _ => 0.0,
        }
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
    fn test_one_word_difference_is_scaled_by_hash_size() {
        let det = VideoDetector::new(0.9);
        let mk = |first: usize| {
            let mut words: Vec<usize> = vec![0; 16];
            words[0] = first;
            format!(
                "{{\"hash\":{:?},\"src_path\":\"/tmp/test.mp4\",\"duration\":0}}",
                words
            )
        };
        // 8 differing bits out of 1000 -> 0.992
        let sim = det.compare_signatures(&mk(0), &mk(0xff));
        assert!((sim - 0.992).abs() < 1e-9, "got {sim}");
    }

    #[test]
    fn test_compare_invalid_signature() {
        let det = VideoDetector::new(0.9);
        let sim = det.compare_signatures("not json", "also not json");
        assert!((sim - 0.0).abs() < f64::EPSILON);
    }
}
