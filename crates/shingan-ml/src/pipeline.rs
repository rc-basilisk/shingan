//! Multi-tier image classification pipeline orchestrator.
//!
//! [`TieredPipeline`] is the main entry point for classifying images.
//!
//! **With the local CLIP model installed**, every image (except tiny icons) is
//! classified by CLIP (Tier 2). The cheap Tier 0 heuristics still contribute:
//! their guess is used as a *prior* that boosts the matching CLIP category
//! (and camera EXIF boosts all `photo_*` categories), so a camera photo ends up
//! as `photo_landscape` or `photo_food` rather than a generic `photo_general`.
//! If CLIP's probability is below
//! [`PipelineConfig::cloud_escalation_threshold`] and a cloud categorizer is
//! attached, the image is escalated to Tier 3.
//!
//! **Without CLIP**, the pipeline falls back to the original escalation:
//! Tier 0 heuristics, Tier 1 structure analysis, then the cloud (if any), then
//! relaxed versions of Tier 0/1.
//!
//! Results below [`PipelineConfig::min_confidence`] are reported as
//! [`ImageSubCategory::Other`].

use crate::cloud::CloudCategorizer;
#[cfg(feature = "onnx")]
use crate::onnx::ClipOnnxClassifier;
use crate::taxonomy::ImageSubCategory;
use crate::tier0::{self, ImageSignals};
use crate::tier1;
use std::path::Path;
use std::sync::Arc;

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
    /// Minimum confidence to accept Tier 0 (0..=1) when CLIP is unavailable.
    pub tier0_min_confidence: f32,
    /// Minimum confidence to accept Tier 1 when CLIP is unavailable.
    pub tier1_min_confidence: f32,
    /// Minimum CLIP probability to accept Tier 2 without asking the cloud.
    pub tier2_min_confidence: f32,
    /// Escalate to cloud when the best local probability is below this.
    pub cloud_escalation_threshold: f32,
    /// Results below this confidence are filed as `other`.
    pub min_confidence: f32,
    /// Upper bound on cloud requests per pipeline (`None` = unlimited).
    pub max_cloud_requests: Option<u32>,
    pub model_dir: Option<std::path::PathBuf>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            tier0_min_confidence: 0.62,
            tier1_min_confidence: 0.48,
            tier2_min_confidence: 0.45,
            cloud_escalation_threshold: 0.4,
            min_confidence: 0.0,
            max_cloud_requests: None,
            model_dir: None,
        }
    }
}

/// How strongly a Tier 0 guess boosts the matching CLIP category.
#[cfg(feature = "onnx")]
const TIER0_PRIOR_WEIGHT: f32 = 3.0;
/// Boost applied to every photo category when camera EXIF is present.
#[cfg(feature = "onnx")]
const EXIF_PHOTO_PRIOR: f32 = 3.0;

/// Multi-tier image classifier (Tier 0-2 local; Tier 3 cloud optional).
pub struct TieredPipeline {
    config: PipelineConfig,
    #[cfg(feature = "onnx")]
    onnx: Option<ClipOnnxClassifier>,
    onnx_error: Option<String>,
    cloud: Option<Arc<dyn CloudCategorizer>>,
    cloud_requests: u32,
    cloud_errors: u32,
}

impl TieredPipeline {
    pub fn new(config: PipelineConfig) -> Self {
        #[cfg(feature = "onnx")]
        let (onnx, onnx_error) = match ClipOnnxClassifier::try_load(config.model_dir.as_deref()) {
            Ok(c) => (c, None),
            Err(e) => (None, Some(e)),
        };
        #[cfg(not(feature = "onnx"))]
        let onnx_error = None;

        Self {
            config,
            #[cfg(feature = "onnx")]
            onnx,
            onnx_error,
            cloud: None,
            cloud_requests: 0,
            cloud_errors: 0,
        }
    }

    /// Attach an optional cloud categorizer for Tier 3 escalation.
    pub fn with_cloud(mut self, cloud: Option<Arc<dyn CloudCategorizer>>) -> Self {
        self.cloud = cloud;
        self
    }

    /// Why the local CLIP model could not be loaded, if it is installed but unusable.
    pub fn onnx_error(&self) -> Option<&str> {
        self.onnx_error.as_deref()
    }

    /// Whether Tier 2 (local CLIP) is active.
    pub fn has_clip(&self) -> bool {
        #[cfg(feature = "onnx")]
        {
            self.onnx.is_some()
        }
        #[cfg(not(feature = "onnx"))]
        {
            false
        }
    }

    /// Number of cloud requests made so far (and how many failed).
    pub fn cloud_usage(&self) -> (u32, u32) {
        (self.cloud_requests, self.cloud_errors)
    }

    fn try_cloud(&mut self, path: &Path, confidence: f32) -> Option<ClassificationResult> {
        let cloud = self.cloud.as_ref()?;
        if self
            .config
            .max_cloud_requests
            .is_some_and(|max| self.cloud_requests >= max)
        {
            return None;
        }
        self.cloud_requests += 1;
        match cloud.categorize_blocking(path) {
            Ok(category) => Some(ClassificationResult {
                category,
                confidence,
                tier: 3,
            }),
            Err(_) => {
                self.cloud_errors += 1;
                None
            }
        }
    }

    fn apply_min_confidence(&self, mut r: ClassificationResult) -> ClassificationResult {
        if r.confidence < self.config.min_confidence {
            r.category = ImageSubCategory::Other;
        }
        r
    }

    /// Classify one image (cloud is used only if attached and needed).
    pub fn classify_local(
        &mut self,
        path: &Path,
        info: &shingan_core::file_info::FileInfo,
    ) -> ClassificationResult {
        let r = self.classify_inner(path, info);
        self.apply_min_confidence(r)
    }

    fn classify_inner(
        &mut self,
        path: &Path,
        info: &shingan_core::file_info::FileInfo,
    ) -> ClassificationResult {
        let signals = ImageSignals::from_core(info);
        let t0 = tier0::classify(&signals);

        #[cfg(feature = "onnx")]
        if let Some(r) = self.classify_with_clip(path, &signals, t0) {
            return r;
        }

        // ---- No CLIP: heuristic escalation ----
        if let Some((cat, conf)) = t0 {
            if conf >= self.config.tier0_min_confidence {
                return ClassificationResult {
                    category: cat,
                    confidence: conf,
                    tier: 0,
                };
            }
        }

        let t1 = tier1::classify(path, None);
        if let Some((cat, conf)) = t1 {
            if conf >= self.config.tier1_min_confidence {
                return ClassificationResult {
                    category: cat,
                    confidence: conf,
                    tier: 1,
                };
            }
        }

        if let Some(r) = self.try_cloud(path, 0.75) {
            return r;
        }

        // Relaxed local fallback
        if let Some((cat, conf)) = t0 {
            return ClassificationResult {
                category: cat,
                confidence: conf * 0.9,
                tier: 0,
            };
        }
        if let Some((cat, conf)) = t1 {
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

    /// Tier 2 with Tier 0 as a prior. Returns `None` if CLIP is unavailable or failed.
    #[cfg(feature = "onnx")]
    fn classify_with_clip(
        &mut self,
        path: &Path,
        signals: &ImageSignals,
        t0: Option<(ImageSubCategory, f32)>,
    ) -> Option<ClassificationResult> {
        // CLIP is unreliable on tiny images; trust the icon heuristic there.
        if let Some((ImageSubCategory::LogoIcon, conf)) = t0 {
            if conf >= self.config.tier0_min_confidence {
                return Some(ClassificationResult {
                    category: ImageSubCategory::LogoIcon,
                    confidence: conf,
                    tier: 0,
                });
            }
        }

        let probs = self.onnx.as_mut()?.probabilities(path).ok()?;
        let posterior = apply_priors(&probs, signals.has_exif, t0);
        let (i, p) = crate::onnx::argmax(&posterior);
        let result = ClassificationResult {
            category: ImageSubCategory::ALL[i],
            confidence: p,
            tier: 2,
        };

        if p < self.config.tier2_min_confidence && p < self.config.cloud_escalation_threshold {
            if let Some(r) = self.try_cloud(path, 0.8) {
                return Some(r);
            }
        }
        Some(result)
    }
}

/// Combine CLIP probabilities with heuristic priors and renormalize.
#[cfg(feature = "onnx")]
fn apply_priors(probs: &[f32], has_exif: bool, t0: Option<(ImageSubCategory, f32)>) -> Vec<f32> {
    let mut post: Vec<f32> = probs.to_vec();
    for (i, cat) in ImageSubCategory::ALL.iter().enumerate() {
        if has_exif && cat.label().starts_with("photo_") {
            post[i] *= EXIF_PHOTO_PRIOR;
        }
        if let Some((c, conf)) = t0 {
            if c == *cat {
                post[i] *= 1.0 + TIER0_PRIOR_WEIGHT * conf;
            }
        }
    }
    let sum: f32 = post.iter().sum::<f32>().max(1e-12);
    post.iter().map(|p| p / sum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shingan_core::file_info::{FileCategory, FileInfo};
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn make_file_info(dims: Option<(u32, u32)>, has_exif: bool, size: u64) -> FileInfo {
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

    struct FixedCloud(ImageSubCategory, std::sync::atomic::AtomicU32);
    impl CloudCategorizer for FixedCloud {
        fn categorize_blocking(&self, _: &Path) -> Result<ImageSubCategory, String> {
            self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(self.0)
        }
    }

    #[test]
    fn cloud_budget_is_respected() {
        let cloud = Arc::new(FixedCloud(ImageSubCategory::Meme, Default::default()));
        let cfg = PipelineConfig {
            max_cloud_requests: Some(2),
            model_dir: Some(PathBuf::from("/nonexistent/models")),
            ..PipelineConfig::default()
        };
        let mut pipeline =
            TieredPipeline::new(cfg).with_cloud(Some(cloud.clone() as Arc<dyn CloudCategorizer>));
        // No dimensions -> no local tier commits -> cloud is asked.
        let info = make_file_info(None, false, 100_000);
        let results: Vec<_> = (0..4)
            .map(|_| pipeline.classify_local(Path::new("/nonexistent/x.png"), &info))
            .collect();
        assert_eq!(results[0].category, ImageSubCategory::Meme);
        assert_eq!(results[0].tier, 3);
        assert_eq!(results[3].category, ImageSubCategory::Other);
        assert_eq!(cloud.1.load(std::sync::atomic::Ordering::Relaxed), 2);
        assert_eq!(pipeline.cloud_usage(), (2, 0));
    }

    #[test]
    fn min_confidence_files_as_other() {
        let cfg = PipelineConfig {
            min_confidence: 0.95,
            model_dir: Some(PathBuf::from("/nonexistent/models")),
            ..PipelineConfig::default()
        };
        let mut pipeline = TieredPipeline::new(cfg);
        let info = make_file_info(Some((1920, 1080)), false, 400_000);
        let r = pipeline.classify_local(Path::new("/tmp/test.png"), &info);
        assert_eq!(r.category, ImageSubCategory::Other);
    }

    #[cfg(feature = "onnx")]
    #[test]
    fn exif_prior_favors_photo_categories() {
        let mut probs = vec![0.0f32; ImageSubCategory::ALL.len()];
        let art = ImageSubCategory::ALL
            .iter()
            .position(|c| *c == ImageSubCategory::Artwork)
            .unwrap();
        let food = ImageSubCategory::ALL
            .iter()
            .position(|c| *c == ImageSubCategory::PhotoFood)
            .unwrap();
        probs[art] = 0.5;
        probs[food] = 0.3;
        let post = apply_priors(&probs, true, None);
        assert!(post[food] > post[art]);
        let post = apply_priors(&probs, false, None);
        assert!(post[art] > post[food]);
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
            ..PipelineConfig::default()
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
            ..PipelineConfig::default()
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
