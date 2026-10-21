use crate::detector::{pigeonhole_keys, Detector, PreparedSignatures};
use crate::error::Result;
use crate::file_info::FileCategory;
use img_hash::image as ih_image;
use img_hash::{HashAlg, HasherConfig, ImageHash};
use std::path::Path;

/// Signature format tag; bump when the hashing scheme changes.
const SIG_VERSION: &str = "v2";

/// Image duplicate detector using three perceptual hashes.
///
/// Each image is decoded once and hashed with an average hash (aHash), a
/// DCT-based perceptual hash (pHash) and a gradient hash (dHash). Two images
/// are as similar as their *least* similar hash, so all three must agree —
/// this keeps false positives low on visually busy images.
pub struct ImageDetector {
    threshold: f64,
    hash_size: u32,
}

type Hashes = (ImageHash, ImageHash, ImageHash);

impl ImageDetector {
    pub fn new(threshold: f64, hash_size: u32) -> Self {
        Self {
            threshold,
            hash_size: hash_size.max(4),
        }
    }

    fn config(&self) -> HasherConfig {
        HasherConfig::new().hash_size(self.hash_size, self.hash_size)
    }

    fn bits(&self) -> usize {
        (self.hash_size * self.hash_size) as usize
    }

    fn parse_signature(sig: &str) -> Option<Hashes> {
        let mut parts = sig.split('|');
        if parts.next()? != SIG_VERSION {
            return None;
        }
        let ahash = ImageHash::from_base64(parts.next()?).ok()?;
        let phash = ImageHash::from_base64(parts.next()?).ok()?;
        let dhash = ImageHash::from_base64(parts.next()?).ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some((ahash, phash, dhash))
    }

    fn similarity_of(&self, a: &Hashes, b: &Hashes) -> f64 {
        let max_dist = self.bits() as f64;
        let sim = |x: &ImageHash, y: &ImageHash| 1.0 - (x.dist(y) as f64 / max_dist).min(1.0);
        sim(&a.0, &b.0).min(sim(&a.1, &b.1)).min(sim(&a.2, &b.2))
    }
}

impl Detector for ImageDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        // Decode once, hash three ways. Undecodable files (SVG, corrupt) are skipped.
        let img = match open_sniffed(path) {
            Some(img) => img,
            None => return Ok(None),
        };
        // Hashers are cheap to build (the DCT one isn't Send/Sync, so it can't be stored).
        let ahash = self
            .config()
            .hash_alg(HashAlg::Mean)
            .to_hasher()
            .hash_image(&img);
        let phash = self
            .config()
            .hash_alg(HashAlg::Mean)
            .preproc_dct()
            .to_hasher()
            .hash_image(&img);
        let dhash = self
            .config()
            .hash_alg(HashAlg::Gradient)
            .to_hasher()
            .hash_image(&img);
        Ok(Some(format!(
            "{SIG_VERSION}|{}|{}|{}",
            ahash.to_base64(),
            phash.to_base64(),
            dhash.to_base64()
        )))
    }

    fn compare_signatures(&self, sig1: &str, sig2: &str) -> f64 {
        match (Self::parse_signature(sig1), Self::parse_signature(sig2)) {
            (Some(a), Some(b)) => self.similarity_of(&a, &b),
            _ => 0.0,
        }
    }

    /// Pigeonhole bands over the dHash: any pair within the allowed Hamming
    /// distance on every hash is guaranteed to share at least one band.
    fn candidate_keys(&self, sig: &str, threshold: f64) -> Vec<u64> {
        let Some((_, _, dhash)) = Self::parse_signature(sig) else {
            return vec![u64::MAX];
        };
        let bits = self.bits();
        let bytes = dhash.as_bytes();
        let bitvec: Vec<bool> = (0..bits.min(bytes.len() * 8))
            .map(|i| bytes[i / 8] & (1 << (i % 8)) != 0)
            .collect();
        let max_dist = ((1.0 - threshold.clamp(0.0, 1.0)) * bits as f64).floor() as usize;
        pigeonhole_keys(&bitvec, max_dist, 0x1a6e)
    }

    fn prepare<'a>(&'a self, sigs: &[&'a str]) -> Box<dyn PreparedSignatures + 'a> {
        Box::new(PreparedImages {
            detector: self,
            parsed: sigs.iter().map(|s| Self::parse_signature(s)).collect(),
        })
    }

    fn category(&self) -> FileCategory {
        FileCategory::Image
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }
}

/// Decode an image by sniffing its content rather than trusting the extension
/// (a PNG saved as `.jpg` is common).
pub fn open_sniffed(path: &Path) -> Option<ih_image::DynamicImage> {
    ih_image::io::Reader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}

struct PreparedImages<'a> {
    detector: &'a ImageDetector,
    parsed: Vec<Option<Hashes>>,
}

impl PreparedSignatures for PreparedImages<'_> {
    fn similarity(&self, a: usize, b: usize) -> f64 {
        match (&self.parsed[a], &self.parsed[b]) {
            (Some(x), Some(y)) => self.detector.similarity_of(x, y),
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image(w: u32, h: u32) -> ih_image::RgbImage {
        ih_image::RgbImage::from_fn(w, h, |x, y| {
            let blob = if (x as i32 - 90).pow(2) + (y as i32 - 70).pow(2) < 1600 {
                200
            } else {
                30
            };
            ih_image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, blob])
        })
    }

    #[test]
    fn resized_copy_is_a_near_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("b.png");
        let img = test_image(256, 192);
        img.save(&a).unwrap();
        ih_image::imageops::resize(&img, 128, 96, ih_image::imageops::FilterType::Triangle)
            .save(&b)
            .unwrap();
        let det = ImageDetector::new(0.9, 12);
        let sa = det.compute_signature(&a).unwrap().unwrap();
        let sb = det.compute_signature(&b).unwrap().unwrap();
        let sim = det.compare_signatures(&sa, &sb);
        assert!(sim >= 0.9, "similarity {sim}");
        let ka = det.candidate_keys(&sa, 0.9);
        let kb = det.candidate_keys(&sb, 0.9);
        assert!(ka.iter().any(|k| kb.contains(k)));
    }

    #[test]
    fn test_parse_invalid_signature() {
        assert!(ImageDetector::parse_signature("not_valid_base64!!!").is_none());
        assert!(ImageDetector::parse_signature("only_one_part").is_none());
        assert!(ImageDetector::parse_signature("").is_none());
        // Old (v1) three-part signatures are rejected.
        assert!(ImageDetector::parse_signature("a|b|c").is_none());
    }

    #[test]
    fn undecodable_file_has_no_signature() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.png");
        std::fs::write(&p, b"definitely not a png").unwrap();
        let det = ImageDetector::new(0.9, 12);
        assert!(det.compute_signature(&p).unwrap().is_none());
    }
}
