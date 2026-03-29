use crate::cache::BoundedCache;
use crate::detector::Detector;
use crate::error::{Error, Result};
use crate::file_info::FileCategory;
use img_hash::image as ih_image;
use img_hash::{HashAlg, HasherConfig, ImageHash};
use parking_lot::Mutex;
use std::path::Path;

pub struct ImageDetector {
    threshold: f64,
    hash_size: u32,
    cache: Mutex<BoundedCache<String, String>>,
    parse_cache: Mutex<BoundedCache<String, (ImageHash, ImageHash, ImageHash)>>,
}

impl ImageDetector {
    pub fn new(threshold: f64, hash_size: u32) -> Self {
        Self {
            threshold,
            hash_size,
            cache: Mutex::new(BoundedCache::new(5000)),
            parse_cache: Mutex::new(BoundedCache::new(10000)),
        }
    }

    fn compute_hash(&self, path: &Path, alg: HashAlg) -> Result<ImageHash> {
        let img = ih_image::open(path).map_err(|e| Error::Signature {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        let hasher = HasherConfig::new()
            .hash_size(self.hash_size, self.hash_size)
            .hash_alg(alg)
            .to_hasher();

        Ok(hasher.hash_image(&img))
    }

    fn parse_signature(sig: &str) -> Option<(ImageHash, ImageHash, ImageHash)> {
        let parts: Vec<&str> = sig.split('|').collect();
        if parts.len() != 3 {
            return None;
        }
        let ahash = ImageHash::from_base64(parts[0]).ok()?;
        let phash = ImageHash::from_base64(parts[1]).ok()?;
        let dhash = ImageHash::from_base64(parts[2]).ok()?;
        Some((ahash, phash, dhash))
    }

    fn get_or_parse(&self, sig: &str) -> Option<(ImageHash, ImageHash, ImageHash)> {
        let key = sig.to_string();
        {
            let mut cache = self.parse_cache.lock();
            if let Some(cached) = cache.get(&key) {
                return Some(cached.clone());
            }
        }

        let parsed = Self::parse_signature(sig)?;

        {
            let mut cache = self.parse_cache.lock();
            cache.put(key, parsed.clone());
        }

        Some(parsed)
    }
}

impl Detector for ImageDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        let key = path.to_string_lossy().to_string();

        {
            let mut cache = self.cache.lock();
            if let Some(sig) = cache.get(&key) {
                return Ok(Some(sig.clone()));
            }
        }

        let ahash = match self.compute_hash(path, HashAlg::Mean) {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let phash = match self.compute_hash(path, HashAlg::Blockhash) {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let dhash = match self.compute_hash(path, HashAlg::Gradient) {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };

        let sig = format!(
            "{}|{}|{}",
            ahash.to_base64(),
            phash.to_base64(),
            dhash.to_base64()
        );

        {
            let mut cache = self.cache.lock();
            cache.put(key, sig.clone());
        }

        Ok(Some(sig))
    }

    fn compare_signatures(&self, sig1: &str, sig2: &str) -> f64 {
        let (ahash1, phash1, dhash1) = match self.get_or_parse(sig1) {
            Some(h) => h,
            None => return 0.0,
        };
        let (ahash2, phash2, dhash2) = match self.get_or_parse(sig2) {
            Some(h) => h,
            None => return 0.0,
        };

        let max_dist = (self.hash_size * self.hash_size) as f64;

        let a_dist = ahash1.dist(&ahash2) as f64;
        let p_dist = phash1.dist(&phash2) as f64;
        let d_dist = dhash1.dist(&dhash2) as f64;

        let a_sim = 1.0 - (a_dist / max_dist);
        let p_sim = 1.0 - (p_dist / max_dist);
        let d_sim = 1.0 - (d_dist / max_dist);

        a_sim.min(p_sim).min(d_sim)
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
        FileCategory::Image
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_signature() {
        let img = ih_image::RgbImage::from_pixel(8, 8, ih_image::Rgb([0u8; 3]));
        let hasher = HasherConfig::new()
            .hash_size(8, 8)
            .hash_alg(HashAlg::Mean)
            .to_hasher();
        let h1 = hasher.hash_image(&img);
        let hasher = HasherConfig::new()
            .hash_size(8, 8)
            .hash_alg(HashAlg::Blockhash)
            .to_hasher();
        let h2 = hasher.hash_image(&img);
        let hasher = HasherConfig::new()
            .hash_size(8, 8)
            .hash_alg(HashAlg::Gradient)
            .to_hasher();
        let h3 = hasher.hash_image(&img);

        let sig = format!("{}|{}|{}", h1.to_base64(), h2.to_base64(), h3.to_base64());

        let result = ImageDetector::parse_signature(&sig);
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_invalid_signature() {
        assert!(ImageDetector::parse_signature("not_valid_base64!!!").is_none());
        assert!(ImageDetector::parse_signature("only_one_part").is_none());
        assert!(ImageDetector::parse_signature("").is_none());
    }
}
