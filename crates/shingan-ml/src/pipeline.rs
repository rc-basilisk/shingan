//! Multi-tier image classification pipeline orchestrator.
//!
//! [`TieredPipeline`] is the main entry point for classifying images. It runs
//! tiers 0 through 3 in order, returning the first result whose confidence
//! exceeds the tier's configured threshold (see [`PipelineConfig`]).
//!
//! The pipeline follows an escalation pattern:
//!
//! 1. **Tier 0** ([`crate::tier0`]) — metadata heuristics (free)
//! 2. **Tier 1** ([`crate::tier1`]) — structural pixel analysis (cheap)
//! 3. **Tier 2** ([`crate::onnx`]) — local ONNX CLIP inference (moderate)
//! 4. **Tier 3** ([`crate::cloud`]) — remote vision API (expensive, optional)
//!
//! If all tiers fail or are unavailable, a relaxed fallback re-runs Tier 0 and
//! Tier 1 with reduced confidence multipliers. As a last resort the pipeline
//! returns [`ImageSubCategory::Other`] with low confidence.
//!
//! [`ClassificationResult`] captures the chosen category, confidence score, and
//! which tier produced the answer.

use crate::taxonomy::ImageSubCategory;
use crate::tier0::{self, ImageSignals};
use crate::tier1;
#[cfg(feature = "onnx")]
use crate::onnx::ClipOnnxClassifier;
use crate::cloud::CloudCategorizer;
use std::path::Path;

/// Result of classifying one image.
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub category: ImageSubCategory,
    pub confidence: f32,
    pub tier: u8,
}

/// Configuration for tier thresholds and ONNX model directory.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Minimum confidence to accept Tier 0 (0..=1).
    pub tier0_min_confidence: f32,
    /// Minimum confidence to accept Tier 1.
    pub tier1_min_confidence: f32,
    /// Minimum confidence to accept Tier 2 (local CLIP).
    pub tier2_min_confidence: f32,
    /// Escalate to cloud when below this after Tier 2.
    pub cloud_escalation_threshold: f32,
    pub model_dir: Option<std::path::PathBuf>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            tier0_min_confidence: 0.62,
            tier1_min_confidence: 0.48,
            tier2_min_confidence: 0.45,
            cloud_escalation_threshold: 0.4,
            model_dir: None,
        }
    }
}

/// Multi-tier image classifier (Tier 0-2 local; Tier 3 cloud optional).
pub struct TieredPipeline {
    config: PipelineConfig,
    #[cfg(feature = "onnx")]
    onnx: Option<ClipOnnxClassifier>,
    cloud: Option<Box<dyn CloudCategorizer>>,
}

impl TieredPipeline {
    pub fn new(config: PipelineConfig) -> Self {
        #[cfg(feature = "onnx")]
        let onnx = ClipOnnxClassifier::try_load(config.model_dir.as_deref())
            .unwrap_or(None);

        Self {
            config,
            #[cfg(feature = "onnx")]
            onnx,
            cloud: None,
        }
    }

    /// Attach an optional cloud categorizer for Tier 3 escalation.
    pub fn with_cloud(mut self, cloud: Box<dyn CloudCategorizer>) -> Self {
        self.cloud = Some(cloud);
        self
    }

    /// Classify using local tiers first, then cloud if needed.
    pub fn classify_local(
        &mut self,
        path: &Path,
        info: &shingan_core::file_info::FileInfo,
    ) -> ClassificationResult {
        let signals = ImageSignals::from_core(info);

        // Tier 0: heuristics
        if let Some((cat, conf)) = tier0::classify(&signals) {
            if conf >= self.config.tier0_min_confidence {
                return ClassificationResult {
                    category: cat,
                    confidence: conf,
                    tier: 0,
                };
            }
        }

        // Tier 1: structure signals
        if let Some((cat, conf)) = tier1::classify(path, None) {
            if conf >= self.config.tier1_min_confidence {
                return ClassificationResult {
                    category: cat,
                    confidence: conf,
                    tier: 1,
                };
            }
        }

        // Tier 2: ONNX CLIP
        #[cfg(feature = "onnx")]
        if let Some(ref mut session) = self.onnx {
            if let Ok((cat, conf)) = session.classify(path) {
                if conf >= self.config.tier2_min_confidence {
                    return ClassificationResult {
                        category: cat,
                        confidence: conf,
                        tier: 2,
                    };
                }
                // Low confidence from ONNX — try cloud if available
                if conf < self.config.cloud_escalation_threshold {
                    if let Some(ref cloud) = self.cloud {
                        if let Ok(cloud_cat) = cloud.categorize_blocking(path) {
                            return ClassificationResult {
                                category: cloud_cat,
                                confidence: 0.8,
                                tier: 3,
                            };
                        }
                    }
                }
                return ClassificationResult {
                    category: cat,
                    confidence: conf,
                    tier: 2,
                };
            }
        }

        // Cloud-only fallback when ONNX model is unavailable
        if let Some(ref cloud) = self.cloud {
            if let Ok(cloud_cat) = cloud.categorize_blocking(path) {
                return ClassificationResult {
                    category: cloud_cat,
                    confidence: 0.75,
                    tier: 3,
                };
            }
        }

        // Relaxed local fallback
        if let Some((cat, conf)) = tier0::classify(&signals) {
            return ClassificationResult {
                category: cat,
                confidence: conf * 0.9,
                tier: 0,
            };
        }
        if let Some((cat, conf)) = tier1::classify(path, None) {
            return ClassificationResult {
                category: cat,
                confidence: conf * 0.85,
                tier: 1,
            };
        }

        ClassificationResult {
            category: ImageSubCategory::Other,
            confidence: 0.2,
            tier: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shingan_core::file_info::{FileCategory, FileInfo};
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn make_file_info(
        dims: Option<(u32, u32)>,
        has_exif: bool,
        size: u64,
    ) -> FileInfo {
        FileInfo {
            path: PathBuf::from("/tmp/test.png"),
            size,
            modified: SystemTime::UNIX_EPOCH,
            name: "test.png".to_string(),
            extension: "png".to_string(),
            category: FileCategory::Image,
            mime_type: Some("image/png".to_string()),
            dimensions: dims,
            has_exif,
            sub_category: None,
            classification_confidence: None,
            classification_tier: None,
        }
    }

    #[test]
    fn pipeline_config_defaults() {
        let cfg = PipelineConfig::default();
        assert!((cfg.tier0_min_confidence - 0.62).abs() < f32::EPSILON);
        assert!((cfg.tier1_min_confidence - 0.48).abs() < f32::EPSILON);
        assert!((cfg.tier2_min_confidence - 0.45).abs() < f32::EPSILON);
        assert!((cfg.cloud_escalation_threshold - 0.4).abs() < f32::EPSILON);
        assert!(cfg.model_dir.is_none());
    }

    #[test]
    fn classification_result_struct_fields() {
        let r = ClassificationResult {
            category: ImageSubCategory::Meme,
            confidence: 0.77,
            tier: 2,
        };
        assert_eq!(r.category, ImageSubCategory::Meme);
        assert!((r.confidence - 0.77).abs() < f32::EPSILON);
        assert_eq!(r.tier, 2);
    }

    #[test]
    fn classification_result_clone() {
        let r = ClassificationResult {
            category: ImageSubCategory::Artwork,
            confidence: 0.5,
            tier: 1,
        };
        let r2 = r.clone();
        assert_eq!(r2.category, r.category);
        assert_eq!(r2.tier, r.tier);
    }

    #[test]
    fn classify_local_desktop_screenshot() {
        let mut pipeline = TieredPipeline::new(PipelineConfig::default());
        let info = make_file_info(Some((1920, 1080)), false, 400_000);
        let result = pipeline.classify_local(Path::new("/tmp/test.png"), &info);
        assert_eq!(result.category, ImageSubCategory::ScreenshotDesktop);
        assert_eq!(result.tier, 0);
        assert!(result.confidence >= 0.62);
    }

    #[test]
    fn classify_local_exif_photo() {
        let mut pipeline = TieredPipeline::new(PipelineConfig::default());
        let info = make_file_info(Some((4000, 3000)), true, 4_000_000);
        let result = pipeline.classify_local(Path::new("/tmp/test.jpg"), &info);
        assert_eq!(result.category, ImageSubCategory::PhotoGeneral);
        assert_eq!(result.tier, 0);
    }

    #[test]
    fn classify_local_logo_icon() {
        let mut pipeline = TieredPipeline::new(PipelineConfig::default());
        let info = make_file_info(Some((64, 64)), false, 4096);
        let result = pipeline.classify_local(Path::new("/tmp/icon.png"), &info);
        assert_eq!(result.category, ImageSubCategory::LogoIcon);
        assert_eq!(result.tier, 0);
        assert!(result.confidence >= 0.62);
    }

    #[test]
    fn classify_local_mobile_screenshot() {
        let mut pipeline = TieredPipeline::new(PipelineConfig::default());
        let info = make_file_info(Some((1080, 2400)), false, 500_000);
        let result = pipeline.classify_local(Path::new("/tmp/mobile.png"), &info);
        assert_eq!(result.category, ImageSubCategory::ScreenshotMobile);
        assert_eq!(result.tier, 0);
    }

    #[test]
    fn classify_local_no_dimensions_falls_through() {
        let mut pipeline = TieredPipeline::new(PipelineConfig::default());
        let info = make_file_info(None, false, 100_000);
        let result = pipeline.classify_local(Path::new("/nonexistent/test.png"), &info);
        assert_eq!(result.category, ImageSubCategory::Other);
        assert_eq!(result.tier, 0);
        assert!(result.confidence < 0.5);
    }

    #[test]
    fn pipeline_new_does_not_panic() {
        let _pipeline = TieredPipeline::new(PipelineConfig::default());
    }

    #[test]
    fn pipeline_with_custom_config() {
        let cfg = PipelineConfig {
            tier0_min_confidence: 0.9,
            tier1_min_confidence: 0.9,
            tier2_min_confidence: 0.9,
            cloud_escalation_threshold: 0.8,
            model_dir: None,
        };
        let mut pipeline = TieredPipeline::new(cfg);
        let info = make_file_info(Some((1920, 1080)), false, 400_000);
        let result = pipeline.classify_local(Path::new("/tmp/test.png"), &info);
        assert_eq!(
            result.category,
            ImageSubCategory::ScreenshotDesktop,
            "should still match at relaxed fallback"
        );
    }

    #[test]
    fn pipeline_high_threshold_uses_relaxed_fallback() {
        let cfg = PipelineConfig {
            tier0_min_confidence: 0.99,
            tier1_min_confidence: 0.99,
            tier2_min_confidence: 0.99,
            cloud_escalation_threshold: 0.99,
            model_dir: None,
        };
        let mut pipeline = TieredPipeline::new(cfg);
        let info = make_file_info(Some((1920, 1080)), false, 400_000);
        let result = pipeline.classify_local(Path::new("/tmp/test.png"), &info);
        assert_eq!(result.category, ImageSubCategory::ScreenshotDesktop);
        assert!(
            result.confidence < 0.75,
            "relaxed fallback should apply 0.9 multiplier"
        );
    }
}
