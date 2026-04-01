//! 24-category image taxonomy for the ML classification pipeline.
//!
//! [`ImageSubCategory`] defines the target label space shared across all pipeline
//! tiers. The categories are grouped into semantic families:
//!
//! - **Screenshots** (3) — desktop, mobile, screen recording frames
//! - **Photos** (8) — landscape, portrait, food, animal, architecture, event, product, general
//! - **Art** (5) — artwork, anime/manga, pixel art, comics, 3D renders
//! - **Memes** (1) — internet memes and reaction images
//! - **Info-dense** (4) — diagrams, infographics/charts, scanned documents, handwritten notes
//! - **Design** (2) — UI mockups, logos/icons
//! - **Fallback** (1) — `Other` for unclassifiable images
//!
//! Each variant provides:
//! - [`ImageSubCategory::label`] — snake_case string safe for use as folder names
//! - [`ImageSubCategory::display_name`] — human-readable name for UI display
//! - [`ImageSubCategory::clip_prompt`] — English text prototype for CLIP zero-shot
//!   cosine similarity (used by [`crate::onnx::ClipOnnxClassifier`])
//!
//! The taxonomy is intentionally fixed at 24 categories to keep the CLIP prototype
//! matrix small and the sorting hierarchy manageable. New categories should only be
//! added when they represent a genuinely distinct visual class that existing
//! categories cannot cover.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSubCategory {
    ScreenshotDesktop,
    ScreenshotMobile,
    ScreenRecordingFrame,
    PhotoLandscape,
    PhotoPortrait,
    PhotoFood,
    PhotoAnimal,
    PhotoArchitecture,
    PhotoEvent,
    PhotoProduct,
    PhotoGeneral,
    Artwork,
    AnimeManga,
    PixelArt,
    Comic,
    Render3d,
    Meme,
    Diagram,
    InfographicChart,
    ScannedDocument,
    Handwritten,
    UiDesign,
    LogoIcon,
    Other,
}

impl ImageSubCategory {
    pub const ALL: [ImageSubCategory; 24] = [
        Self::ScreenshotDesktop,
        Self::ScreenshotMobile,
        Self::ScreenRecordingFrame,
        Self::PhotoLandscape,
        Self::PhotoPortrait,
        Self::PhotoFood,
        Self::PhotoAnimal,
        Self::PhotoArchitecture,
        Self::PhotoEvent,
        Self::PhotoProduct,
        Self::PhotoGeneral,
        Self::Artwork,
        Self::AnimeManga,
        Self::PixelArt,
        Self::Comic,
        Self::Render3d,
        Self::Meme,
        Self::Diagram,
        Self::InfographicChart,
        Self::ScannedDocument,
        Self::Handwritten,
        Self::UiDesign,
        Self::LogoIcon,
        Self::Other,
    ];

    /// Folder-safe snake_case label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::ScreenshotDesktop => "screenshot_desktop",
            Self::ScreenshotMobile => "screenshot_mobile",
            Self::ScreenRecordingFrame => "screen_recording_frame",
            Self::PhotoLandscape => "photo_landscape",
            Self::PhotoPortrait => "photo_portrait",
            Self::PhotoFood => "photo_food",
            Self::PhotoAnimal => "photo_animal",
            Self::PhotoArchitecture => "photo_architecture",
            Self::PhotoEvent => "photo_event",
            Self::PhotoProduct => "photo_product",
            Self::PhotoGeneral => "photo_general",
            Self::Artwork => "artwork",
            Self::AnimeManga => "anime_manga",
            Self::PixelArt => "pixel_art",
            Self::Comic => "comic",
            Self::Render3d => "render_3d",
            Self::Meme => "meme",
            Self::Diagram => "diagram",
            Self::InfographicChart => "infographic_chart",
            Self::ScannedDocument => "scanned_document",
            Self::Handwritten => "handwritten",
            Self::UiDesign => "ui_design",
            Self::LogoIcon => "logo_icon",
            Self::Other => "other",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.label() == s)
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::ScreenshotDesktop => "Desktop screenshot",
            Self::ScreenshotMobile => "Mobile screenshot",
            Self::ScreenRecordingFrame => "Screen recording frame",
            Self::PhotoLandscape => "Landscape photo",
            Self::PhotoPortrait => "Portrait / people photo",
            Self::PhotoFood => "Food photo",
            Self::PhotoAnimal => "Animal photo",
            Self::PhotoArchitecture => "Architecture photo",
            Self::PhotoEvent => "Event / group photo",
            Self::PhotoProduct => "Product / object photo",
            Self::PhotoGeneral => "Photo (general)",
            Self::Artwork => "Artwork",
            Self::AnimeManga => "Anime / manga art",
            Self::PixelArt => "Pixel art",
            Self::Comic => "Comic / strip",
            Self::Render3d => "3D render",
            Self::Meme => "Meme",
            Self::Diagram => "Diagram",
            Self::InfographicChart => "Infographic / chart",
            Self::ScannedDocument => "Scanned document",
            Self::Handwritten => "Handwritten / whiteboard",
            Self::UiDesign => "UI design",
            Self::LogoIcon => "Logo / icon",
            Self::Other => "Other",
        }
    }

    /// Text prototype for CLIP-style zero-shot similarity (English).
    pub fn clip_prompt(&self) -> &'static str {
        match self {
            Self::ScreenshotDesktop => {
                "a screenshot of a desktop computer screen showing applications or a web browser"
            }
            Self::ScreenshotMobile => {
                "a screenshot of a mobile phone or tablet user interface"
            }
            Self::ScreenRecordingFrame => {
                "a still frame from a screen recording or tutorial video capture"
            }
            Self::PhotoLandscape => {
                "a photograph of a natural landscape, scenery, mountains, ocean, forest, or travel vista"
            }
            Self::PhotoPortrait => {
                "a photograph focused on people, faces, portraits, selfies, or headshots"
            }
            Self::PhotoFood => "a photograph of food, meals, drinks, or restaurant dishes",
            Self::PhotoAnimal => "a photograph of animals, pets, or wildlife",
            Self::PhotoArchitecture => {
                "a photograph of buildings, architecture, city streets, or real estate interiors"
            }
            Self::PhotoEvent => {
                "a photograph of a group at an event, party, concert, wedding, or gathering"
            }
            Self::PhotoProduct => {
                "a product photograph, macro shot, still life, or object on a plain background"
            }
            Self::PhotoGeneral => "a general photograph not fitting other categories",
            Self::Artwork => {
                "a digital painting, illustration, or concept art in a non-anime style"
            }
            Self::AnimeManga => {
                "an anime or manga style illustration, fan art, or visual novel artwork"
            }
            Self::PixelArt => "pixel art, retro game sprites, or low-resolution stylized graphics",
            Self::Comic => "a comic page, webcomic strip, or sequential panel artwork",
            Self::Render3d => "a 3D computer render, CGI scene, or Blender output",
            Self::Meme => "an internet meme, reaction image, or humorous captioned image",
            Self::Diagram => {
                "a technical diagram, flowchart, schematic, mind map, or UML diagram"
            }
            Self::InfographicChart => {
                "an infographic, data visualization, bar chart, line chart, or business graph"
            }
            Self::ScannedDocument => {
                "a scanned document page, receipt, invoice, or photographed text document"
            }
            Self::Handwritten => {
                "handwritten notes on paper, a whiteboard photo, or sketch on paper"
            }
            Self::UiDesign => {
                "a user interface or UX mockup, wireframe, or design composition"
            }
            Self::LogoIcon => "a logo, app icon, favicon, or small symbolic graphic",
            Self::Other => "an image that does not fit other categories",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_constant_has_24_variants() {
        assert_eq!(ImageSubCategory::ALL.len(), 24);
    }

    #[test]
    fn all_variants_are_unique() {
        let set: HashSet<ImageSubCategory> = ImageSubCategory::ALL.iter().copied().collect();
        assert_eq!(set.len(), ImageSubCategory::ALL.len());
    }

    #[test]
    fn label_round_trip_every_variant() {
        for cat in &ImageSubCategory::ALL {
            let label = cat.label();
            let recovered = ImageSubCategory::from_label(label);
            assert_eq!(recovered, Some(*cat), "round-trip failed for {label}");
        }
    }

    #[test]
    fn from_label_unknown_returns_none() {
        assert_eq!(ImageSubCategory::from_label("nonexistent_category"), None);
        assert_eq!(ImageSubCategory::from_label(""), None);
    }

    #[test]
    fn labels_are_snake_case() {
        for cat in &ImageSubCategory::ALL {
            let label = cat.label();
            assert!(
                label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                "label {label:?} is not snake_case"
            );
        }
    }

    #[test]
    fn labels_are_unique() {
        let labels: Vec<&str> = ImageSubCategory::ALL.iter().map(|c| c.label()).collect();
        let set: HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(set.len(), labels.len());
    }

    #[test]
    fn display_name_non_empty_for_every_variant() {
        for cat in &ImageSubCategory::ALL {
            let name = cat.display_name();
            assert!(!name.is_empty(), "empty display_name for {:?}", cat);
            assert!(name.len() >= 3, "suspiciously short display_name: {name}");
        }
    }

    #[test]
    fn clip_prompt_non_empty_for_every_variant() {
        for cat in &ImageSubCategory::ALL {
            let prompt = cat.clip_prompt();
            assert!(!prompt.is_empty(), "empty clip_prompt for {:?}", cat);
            assert!(
                prompt.len() >= 10,
                "clip_prompt too short for {:?}: {prompt}",
                cat
            );
        }
    }

    #[test]
    fn serde_json_round_trip_every_variant() {
        for cat in &ImageSubCategory::ALL {
            let json = serde_json::to_string(cat).unwrap();
            let recovered: ImageSubCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(recovered, *cat, "serde round-trip failed for {:?}", cat);
        }
    }

    #[test]
    fn serde_uses_snake_case() {
        let json = serde_json::to_string(&ImageSubCategory::ScreenshotDesktop).unwrap();
        assert_eq!(json, "\"screenshot_desktop\"");

        let json = serde_json::to_string(&ImageSubCategory::Render3d).unwrap();
        assert_eq!(json, "\"render3d\"");

        let json = serde_json::to_string(&ImageSubCategory::AnimeManga).unwrap();
        assert_eq!(json, "\"anime_manga\"");
    }

    #[test]
    fn serde_deserialize_from_string() {
        let cat: ImageSubCategory = serde_json::from_str("\"photo_landscape\"").unwrap();
        assert_eq!(cat, ImageSubCategory::PhotoLandscape);

        let cat: ImageSubCategory = serde_json::from_str("\"logo_icon\"").unwrap();
        assert_eq!(cat, ImageSubCategory::LogoIcon);
    }

    #[test]
    fn serde_invalid_label_fails() {
        let result = serde_json::from_str::<ImageSubCategory>("\"not_a_real_category\"");
        assert!(result.is_err());
    }

    #[test]
    fn specific_label_values() {
        assert_eq!(
            ImageSubCategory::ScreenshotDesktop.label(),
            "screenshot_desktop"
        );
        assert_eq!(ImageSubCategory::PhotoGeneral.label(), "photo_general");
        assert_eq!(ImageSubCategory::Render3d.label(), "render_3d");
        assert_eq!(ImageSubCategory::LogoIcon.label(), "logo_icon");
        assert_eq!(ImageSubCategory::Other.label(), "other");
    }

    #[test]
    fn specific_display_names() {
        assert_eq!(
            ImageSubCategory::ScreenshotDesktop.display_name(),
            "Desktop screenshot"
        );
        assert_eq!(ImageSubCategory::Render3d.display_name(), "3D render");
        assert_eq!(ImageSubCategory::Other.display_name(), "Other");
    }
}
