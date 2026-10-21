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
#[cfg(any(feature = "document-detect", feature = "code-detect"))]
pub mod minhash;
#[cfg(feature = "video-detect")]
pub mod video;

/// A batch of signatures prepared (parsed) once for fast pairwise comparison.
///
/// Indices refer to positions in the slice passed to [`Detector::prepare`].
pub trait PreparedSignatures: Send + Sync {
    /// Similarity between signatures `a` and `b`, in `0.0..=1.0`.
    fn similarity(&self, a: usize, b: usize) -> f64;
}

/// Trait implemented by all file-type-specific duplicate detectors.
///
/// Each detector can compute signatures for files of its category and
/// compare them for similarity.
pub trait Detector: Send + Sync {
    /// Compute a signature/fingerprint for the file.
    /// Returns `Ok(None)` if the file cannot be processed (corrupt, empty, etc.).
    fn compute_signature(&self, path: &Path) -> Result<Option<String>>;

    /// Compare two files directly and return a similarity score in `0.0..=1.0`.
    fn compare_files(&self, file1: &Path, file2: &Path) -> Result<f64> {
        let sig1 = self.compute_signature(file1)?;
        let sig2 = self.compute_signature(file2)?;
        match (sig1, sig2) {
            (Some(s1), Some(s2)) => Ok(self.compare_signatures(&s1, &s2)),
            _ => Ok(0.0),
        }
    }

    /// Compare two pre-computed signatures. Returns similarity in `0.0..=1.0`.
    /// Default implementation: exact match (1.0) or no match (0.0).
    #[must_use]
    fn compare_signatures(&self, sig1: &str, sig2: &str) -> f64 {
        if sig1 == sig2 {
            1.0
        } else {
            0.0
        }
    }

    /// Locality-sensitive bucket keys for candidate generation.
    ///
    /// Two signatures are only compared if they share at least one key, so a
    /// detector must guarantee (or make very likely) that any pair whose
    /// similarity is `>= threshold` shares a key. The default returns a single
    /// key derived from the whole signature, which is correct for exact-match
    /// detectors. Returning the same constant key for every signature means
    /// "compare everything with everything".
    fn candidate_keys(&self, sig: &str, _threshold: f64) -> Vec<u64> {
        vec![stable_hash64(sig.as_bytes())]
    }

    /// Parse a batch of signatures once so that pairwise comparisons are cheap.
    /// The default wraps [`Detector::compare_signatures`].
    fn prepare<'a>(&'a self, sigs: &[&'a str]) -> Box<dyn PreparedSignatures + 'a> {
        Box::new(RawPrepared {
            detector: self,
            sigs: sigs.to_vec(),
        })
    }

    /// True when only byte-identical files can match. The scanner then skips
    /// files whose size is unique, since they cannot have a duplicate.
    fn exact_only(&self) -> bool {
        false
    }

    /// The file category this detector handles.
    fn category(&self) -> FileCategory;

    /// The similarity threshold for this detector.
    fn threshold(&self) -> f64;
}

struct RawPrepared<'a, D: Detector + ?Sized> {
    detector: &'a D,
    sigs: Vec<&'a str>,
}

impl<D: Detector + ?Sized> PreparedSignatures for RawPrepared<'_, D> {
    fn similarity(&self, a: usize, b: usize) -> f64 {
        self.detector.compare_signatures(self.sigs[a], self.sigs[b])
    }
}

/// FNV-1a 64-bit hash. Stable across Rust versions and platforms, which matters
/// because derived values end up persisted in the signature cache.
pub fn stable_hash64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// SplitMix64 finalizer: a cheap, well-distributed 64-bit mixer.
pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Pigeonhole LSH over a bit string: if two bit strings of length `bits.len()`
/// differ in at most `max_dist` positions, splitting them into `max_dist + 1`
/// contiguous bands guarantees at least one band is identical. Returns one key
/// per band. When the bands would be too small to be selective, returns a
/// single constant key (compare everything).
pub fn pigeonhole_keys(bits: &[bool], max_dist: usize, salt: u64) -> Vec<u64> {
    let n_bands = max_dist + 1;
    // Bands shorter than ~4 bits barely narrow the search; fall back to brute force.
    if bits.is_empty() || bits.len() / n_bands < 4 {
        return vec![salt];
    }
    let mut keys = Vec::with_capacity(n_bands);
    for band in 0..n_bands {
        let start = band * bits.len() / n_bands;
        let end = (band + 1) * bits.len() / n_bands;
        let mut h = mix64(salt ^ (band as u64).wrapping_mul(0x1000_0000_01b3));
        for (i, &bit) in bits[start..end].iter().enumerate() {
            if bit {
                h = mix64(h ^ (i as u64 + 1));
            } else {
                h = mix64(h.wrapping_add(i as u64 + 1));
            }
        }
        keys.push(h);
    }
    keys
}

/// Compute SHA-256 hash of a file, streaming in chunks.
pub fn file_sha256(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| crate::error::Error::io(path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pigeonhole_guarantees_shared_band() {
        let a: Vec<bool> = (0..144).map(|i| i % 3 == 0).collect();
        let mut b = a.clone();
        // Flip 7 bits spread across the string.
        for i in [1, 20, 40, 60, 80, 100, 140] {
            b[i] = !b[i];
        }
        let ka = pigeonhole_keys(&a, 7, 1);
        let kb = pigeonhole_keys(&b, 7, 1);
        assert_eq!(ka.len(), 8);
        assert!(ka.iter().zip(&kb).any(|(x, y)| x == y));
    }

    #[test]
    fn pigeonhole_falls_back_to_constant() {
        let a = vec![true; 16];
        assert_eq!(pigeonhole_keys(&a, 10, 42), vec![42]);
    }

    #[test]
    fn stable_hash_is_stable() {
        assert_eq!(stable_hash64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(stable_hash64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
