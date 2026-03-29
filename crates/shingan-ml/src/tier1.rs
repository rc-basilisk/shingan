//! Tier 1: structural signal classification (~5 ms per image).
//!
//! This tier reads a sub-sampled version of the image to compute lightweight
//! statistics — mean luma (brightness) and edge energy — without running a full
//! neural network. These signals are effective at separating:
//!
//! - **Diagrams/schematics** — high edge density with moderate contrast
//! - **Scanned documents** — very bright background with low edge energy
//!
//! The module also provides [`perceptual_hash_b64`] for computing a quick
//! Blockhash perceptual hash, which can be used as a routing key or
//! deduplication signal by higher layers.
//!
//! When neither rule fires, `classify` returns `None` and the pipeline escalates
//! to Tier 2 ([`crate::onnx`]).

use crate::taxonomy::ImageSubCategory;
use img_hash::image as ih_image;
use img_hash::{HashAlg, HasherConfig};
use std::path::Path;

/// Low-cost image structure statistics (brightness, edge energy, hash parameters).
#[derive(Debug, Clone)]
pub struct StructureStats {
    /// Average pixel brightness normalized to `[0, 1]`.
    pub mean_luma: f32,
    /// Edge energy normalized to `[0, 1]`.
    pub edge_energy: f32,
    /// Perceptual hash grid size (e.g. 8 for an 8x8 hash).
    pub hash_size: u32,
}

fn sample_luma_and_edges(path: &Path) -> Option<StructureStats> {
    let img = ih_image::open(path).ok()?.to_rgb8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let mut sum = 0u64;
    let mut edge_sum = 0u64;
    let step = 2usize;
    for y in (0..h as usize).step_by(step) {
        for x in (0..w as usize).step_by(step) {
            let p = img.get_pixel(x as u32, y as u32);
            let l = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) as u32;
            sum += l as u64;
        }
    }
    let n = ((w as usize / step + 1) * (h as usize / step + 1)) as f64;
    let mean = sum as f64 / n;

    for y in (step..h as usize - step).step_by(step * 2) {
        for x in (step..w as usize - step).step_by(step * 2) {
            let c = img.get_pixel(x as u32, y as u32);
            let r = img.get_pixel((x + step) as u32, y as u32);
            let d = (c[0] as i32 - r[0] as i32).abs()
                + (c[1] as i32 - r[1] as i32).abs()
                + (c[2] as i32 - r[2] as i32).abs();
            edge_sum += d as u64;
        }
    }
    let edge_n = (((w as usize / step).saturating_sub(2)) * ((h as usize / step).saturating_sub(2)))
        .max(1) as f64;
    let edge_energy = (edge_sum as f64 / edge_n / 765.0) as f32;

    Some(StructureStats {
        mean_luma: (mean / 255.0) as f32,
        edge_energy,
        hash_size: 8,
    })
}

/// Quick pHash for routing (single hash).
pub fn perceptual_hash_b64(path: &Path, hash_size: u32) -> Option<String> {
    let img = ih_image::open(path).ok()?;
    let hasher = HasherConfig::new()
        .hash_size(hash_size, hash_size)
        .hash_alg(HashAlg::Blockhash)
        .to_hasher();
    let h = hasher.hash_image(&img);
    Some(h.to_base64())
}

/// Tier 1 classification when Tier 0 did not commit.
pub fn classify(path: &Path, stats: Option<StructureStats>) -> Option<(ImageSubCategory, f32)> {
    let st = stats.or_else(|| sample_luma_and_edges(path))?;
    // High edge density + moderate contrast → diagram / schematic
    if st.edge_energy > 0.35 && st.mean_luma > 0.2 && st.mean_luma < 0.92 {
        return Some((ImageSubCategory::Diagram, 0.45));
    }
    // Very bright, lower edges → possible document scan
    if st.mean_luma > 0.85 && st.edge_energy < 0.2 {
        return Some((ImageSubCategory::ScannedDocument, 0.4));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dummy_path() -> PathBuf {
        PathBuf::from("/nonexistent/placeholder.png")
    }

    #[test]
    fn high_edge_moderate_luma_is_diagram() {
        let stats = StructureStats {
            mean_luma: 0.5,
            edge_energy: 0.4,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert_eq!(result.map(|r| r.0), Some(ImageSubCategory::Diagram));
    }

    #[test]
    fn diagram_boundary_edge_energy() {
        let stats = StructureStats {
            mean_luma: 0.5,
            edge_energy: 0.36,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert_eq!(result.map(|r| r.0), Some(ImageSubCategory::Diagram));
    }

    #[test]
    fn diagram_requires_luma_above_threshold() {
        let stats = StructureStats {
            mean_luma: 0.15,
            edge_energy: 0.5,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert_eq!(result.map(|r| r.0), None, "luma too low for diagram");
    }

    #[test]
    fn diagram_requires_luma_below_ceiling() {
        let stats = StructureStats {
            mean_luma: 0.95,
            edge_energy: 0.5,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert_ne!(
            result.map(|r| r.0),
            Some(ImageSubCategory::Diagram),
            "luma too high for diagram"
        );
    }

    #[test]
    fn bright_low_edge_is_scanned_document() {
        let stats = StructureStats {
            mean_luma: 0.9,
            edge_energy: 0.1,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert_eq!(result.map(|r| r.0), Some(ImageSubCategory::ScannedDocument));
    }

    #[test]
    fn scanned_document_boundary_luma() {
        let stats = StructureStats {
            mean_luma: 0.86,
            edge_energy: 0.05,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert_eq!(result.map(|r| r.0), Some(ImageSubCategory::ScannedDocument));
    }

    #[test]
    fn not_scanned_document_if_edges_too_high() {
        let stats = StructureStats {
            mean_luma: 0.9,
            edge_energy: 0.25,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert_ne!(result.map(|r| r.0), Some(ImageSubCategory::ScannedDocument));
    }

    #[test]
    fn not_scanned_document_if_luma_too_low() {
        let stats = StructureStats {
            mean_luma: 0.7,
            edge_energy: 0.1,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert_eq!(result.map(|r| r.0), None);
    }

    #[test]
    fn low_edge_low_luma_returns_none() {
        let stats = StructureStats {
            mean_luma: 0.3,
            edge_energy: 0.1,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert!(result.is_none());
    }

    #[test]
    fn moderate_everything_returns_none() {
        let stats = StructureStats {
            mean_luma: 0.5,
            edge_energy: 0.25,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert!(result.is_none());
    }

    #[test]
    fn diagram_takes_priority_over_scanned_document() {
        let stats = StructureStats {
            mean_luma: 0.88,
            edge_energy: 0.4,
            hash_size: 8,
        };
        let result = classify(&dummy_path(), Some(stats));
        assert_eq!(
            result.map(|r| r.0),
            Some(ImageSubCategory::Diagram),
            "diagram branch checked first"
        );
    }

    #[test]
    fn confidence_values_are_valid() {
        let cases = vec![
            StructureStats { mean_luma: 0.5, edge_energy: 0.4, hash_size: 8 },
            StructureStats { mean_luma: 0.9, edge_energy: 0.1, hash_size: 8 },
        ];
        for stats in cases {
            if let Some((_, conf)) = classify(&dummy_path(), Some(stats)) {
                assert!(conf > 0.0 && conf <= 1.0, "confidence {conf} out of range");
            }
        }
    }

    #[test]
    fn none_stats_and_nonexistent_path_returns_none() {
        let result = classify(&dummy_path(), None);
        assert!(result.is_none(), "nonexistent file with no stats should return None");
    }
}
