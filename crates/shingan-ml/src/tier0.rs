//! Tier 0: metadata-only heuristic classification (~0 ms per image).
//!
//! This is the cheapest tier in the pipeline. It examines only the signals
//! already available in [`shingan_core::file_info::FileInfo`] after enrichment:
//! image dimensions, EXIF presence, and file size. No pixel data is read.
//!
//! The heuristics are intentionally conservative — they return `Some` only when
//! the signal is strong enough to classify with reasonable confidence:
//!
//! - **Logo/icon** — very small images (≤256px, <512 KB)
//! - **Camera photo** — EXIF with camera make/model/GPS/lens data
//! - **Desktop screenshot** — exact match against common monitor resolutions, or
//!   landscape aspect ≥1280px wide without EXIF
//! - **Mobile screenshot** — portrait 9:16–9:21 aspect without EXIF
//!
//! When no rule fires, `classify` returns `None` and the pipeline escalates to
//! [`crate::tier1`].

use crate::taxonomy::ImageSubCategory;

/// Inputs derived from [`shingan_core::file_info::FileInfo`] enrichment.
#[derive(Debug, Clone)]
pub struct ImageSignals {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub has_exif: bool,
    pub file_size: u64,
}

impl ImageSignals {
    pub fn from_core(info: &shingan_core::file_info::FileInfo) -> Self {
        Self {
            width: info.dimensions.map(|d| d.0),
            height: info.dimensions.map(|d| d.1),
            has_exif: info.has_exif,
            file_size: info.size,
        }
    }
}

const DESKTOP_RESOLUTIONS: &[(u32, u32)] = &[
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
    (1366, 768),
    (1440, 900),
    (1680, 1050),
    (1920, 1200),
    (2560, 1600),
    (3440, 1440),
    (5120, 2880),
    (1280, 720),
    (1600, 900),
    (2560, 1080),
];

fn matches_desktop_resolution(w: u32, h: u32) -> bool {
    DESKTOP_RESOLUTIONS
        .iter()
        .any(|&(a, b)| (w, h) == (a, b) || (w, h) == (b, a))
}

/// Very small square-ish images are often icons.
fn likely_logo_icon(w: u32, h: u32, size: u64) -> bool {
    let max_dim = w.max(h);
    max_dim <= 256 && size < 512 * 1024
}

/// Portrait phone/tablet aspect (~9:16 to 9:21).
fn likely_mobile_aspect(w: u32, h: u32) -> bool {
    let (short, long) = if w <= h { (w, h) } else { (h, w) };
    if short == 0 {
        return false;
    }
    let r = long as f32 / short as f32;
    (1.7..=2.3).contains(&r) && h > w
}

/// Landscape monitor aspect.
fn likely_desktop_aspect(w: u32, h: u32) -> bool {
    let (short, long) = if w <= h { (w, h) } else { (h, w) };
    if short == 0 {
        return false;
    }
    let r = long as f32 / short as f32;
    w > h && (1.5..=2.4).contains(&r)
}

/// Returns `(category, confidence)` for strong heuristic matches.
pub fn classify(signals: &ImageSignals) -> Option<(ImageSubCategory, f32)> {
    let (w, h) = match (signals.width, signals.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        _ => return None,
    };

    if likely_logo_icon(w, h, signals.file_size) {
        return Some((ImageSubCategory::LogoIcon, 0.85));
    }

    if signals.has_exif {
        // Strong signal for camera photo; sub-type needs ML or Tier 1.
        return Some((ImageSubCategory::PhotoGeneral, 0.65));
    }

    if !signals.has_exif {
        if matches_desktop_resolution(w, h) || (likely_desktop_aspect(w, h) && w >= 1280) {
            return Some((ImageSubCategory::ScreenshotDesktop, 0.75));
        }
        if likely_mobile_aspect(w, h) {
            return Some((ImageSubCategory::ScreenshotMobile, 0.72));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_screenshot_resolution() {
        let s = ImageSignals {
            width: Some(1920),
            height: Some(1080),
            has_exif: false,
            file_size: 400_000,
        };
        let r = classify(&s);
        assert_eq!(r.map(|x| x.0), Some(ImageSubCategory::ScreenshotDesktop));
    }

    #[test]
    fn exif_implies_photo() {
        let s = ImageSignals {
            width: Some(4000),
            height: Some(3000),
            has_exif: true,
            file_size: 4_000_000,
        };
        let r = classify(&s);
        assert_eq!(r.map(|x| x.0), Some(ImageSubCategory::PhotoGeneral));
    }

    #[test]
    fn mobile_screenshot_portrait() {
        let s = ImageSignals {
            width: Some(1080),
            height: Some(2400),
            has_exif: false,
            file_size: 500_000,
        };
        let r = classify(&s);
        assert_eq!(r.map(|x| x.0), Some(ImageSubCategory::ScreenshotMobile));
    }

    #[test]
    fn mobile_screenshot_tall_aspect() {
        let s = ImageSignals {
            width: Some(1080),
            height: Some(2340),
            has_exif: false,
            file_size: 600_000,
        };
        let r = classify(&s);
        assert_eq!(r.map(|x| x.0), Some(ImageSubCategory::ScreenshotMobile));
    }

    #[test]
    fn logo_icon_small_square() {
        let s = ImageSignals {
            width: Some(64),
            height: Some(64),
            has_exif: false,
            file_size: 4096,
        };
        let r = classify(&s);
        assert_eq!(r.map(|x| x.0), Some(ImageSubCategory::LogoIcon));
    }

    #[test]
    fn logo_icon_256x256() {
        let s = ImageSignals {
            width: Some(256),
            height: Some(256),
            has_exif: false,
            file_size: 100_000,
        };
        let r = classify(&s);
        assert_eq!(r.map(|x| x.0), Some(ImageSubCategory::LogoIcon));
    }

    #[test]
    fn logo_icon_not_if_too_large_dim() {
        let s = ImageSignals {
            width: Some(512),
            height: Some(512),
            has_exif: false,
            file_size: 4096,
        };
        let r = classify(&s);
        assert_ne!(r.map(|x| x.0), Some(ImageSubCategory::LogoIcon));
    }

    #[test]
    fn logo_icon_not_if_file_too_big() {
        let s = ImageSignals {
            width: Some(128),
            height: Some(128),
            has_exif: false,
            file_size: 600 * 1024,
        };
        let r = classify(&s);
        assert_ne!(r.map(|x| x.0), Some(ImageSubCategory::LogoIcon));
    }

    #[test]
    fn exif_overrides_desktop_resolution() {
        let s = ImageSignals {
            width: Some(1920),
            height: Some(1080),
            has_exif: true,
            file_size: 2_000_000,
        };
        let r = classify(&s);
        assert_eq!(r.map(|x| x.0), Some(ImageSubCategory::PhotoGeneral));
    }

    #[test]
    fn small_exif_logo_wins_over_exif() {
        let s = ImageSignals {
            width: Some(100),
            height: Some(100),
            has_exif: true,
            file_size: 5_000,
        };
        let r = classify(&s);
        assert_eq!(
            r.map(|x| x.0),
            Some(ImageSubCategory::LogoIcon),
            "logo_icon should win before EXIF check"
        );
    }

    #[test]
    fn none_dimensions_returns_none() {
        let s = ImageSignals {
            width: None,
            height: None,
            has_exif: false,
            file_size: 100_000,
        };
        assert!(classify(&s).is_none());
    }

    #[test]
    fn zero_width_returns_none() {
        let s = ImageSignals {
            width: Some(0),
            height: Some(1080),
            has_exif: false,
            file_size: 100_000,
        };
        assert!(classify(&s).is_none());
    }

    #[test]
    fn zero_height_returns_none() {
        let s = ImageSignals {
            width: Some(1920),
            height: Some(0),
            has_exif: false,
            file_size: 100_000,
        };
        assert!(classify(&s).is_none());
    }

    #[test]
    fn partial_dimensions_returns_none() {
        let s = ImageSignals {
            width: Some(1920),
            height: None,
            has_exif: false,
            file_size: 100_000,
        };
        assert!(classify(&s).is_none());
    }

    #[test]
    fn desktop_all_known_resolutions() {
        for &(w, h) in DESKTOP_RESOLUTIONS {
            let s = ImageSignals {
                width: Some(w),
                height: Some(h),
                has_exif: false,
                file_size: 400_000,
            };
            let r = classify(&s);
            assert_eq!(
                r.map(|x| x.0),
                Some(ImageSubCategory::ScreenshotDesktop),
                "expected desktop screenshot for {w}x{h}"
            );
        }
    }

    #[test]
    fn desktop_resolution_reversed_still_matches_desktop() {
        let s = ImageSignals {
            width: Some(1080),
            height: Some(1920),
            has_exif: false,
            file_size: 400_000,
        };
        let r = classify(&s);
        assert_eq!(
            r.map(|x| x.0),
            Some(ImageSubCategory::ScreenshotDesktop),
            "reversed desktop resolution still matches as desktop"
        );
    }

    #[test]
    fn wide_non_standard_desktop_aspect() {
        let s = ImageSignals {
            width: Some(1600),
            height: Some(900),
            has_exif: false,
            file_size: 400_000,
        };
        let r = classify(&s);
        assert_eq!(r.map(|x| x.0), Some(ImageSubCategory::ScreenshotDesktop));
    }

    #[test]
    fn square_image_no_exif_returns_none() {
        let s = ImageSignals {
            width: Some(1000),
            height: Some(1000),
            has_exif: false,
            file_size: 500_000,
        };
        assert!(
            classify(&s).is_none(),
            "square 1000x1000 no-exif should be indeterminate at tier 0"
        );
    }

    #[test]
    fn confidence_values_in_valid_range() {
        let test_cases = vec![
            ImageSignals {
                width: Some(1920),
                height: Some(1080),
                has_exif: false,
                file_size: 400_000,
            },
            ImageSignals {
                width: Some(4000),
                height: Some(3000),
                has_exif: true,
                file_size: 4_000_000,
            },
            ImageSignals {
                width: Some(1080),
                height: Some(1920),
                has_exif: false,
                file_size: 500_000,
            },
            ImageSignals {
                width: Some(64),
                height: Some(64),
                has_exif: false,
                file_size: 4096,
            },
        ];
        for s in &test_cases {
            if let Some((_, conf)) = classify(s) {
                assert!(conf > 0.0 && conf <= 1.0, "confidence {conf} out of range");
            }
        }
    }
}
