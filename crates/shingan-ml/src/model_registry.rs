//! Registry of downloadable ML models.
//!
//! Each [`ModelDef`] describes a model package — a logical unit composed of one
//! or more files that must all be present for the model to work.  The
//! [`DEFAULT_MODELS`] constant lists every model that ships as a built-in option
//! in the GUI's model manager.

use std::path::{Path, PathBuf};

/// A single file within a model package.
#[derive(Debug, Clone)]
pub struct ModelFileDef {
    pub filename: &'static str,
    /// Download URL. Empty string means the file is not yet hosted.
    pub url: &'static str,
}

/// A downloadable model definition.
#[derive(Debug, Clone)]
pub struct ModelDef {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub files: &'static [ModelFileDef],
    /// Human-readable total size (e.g. "~390 MB").
    pub size_label: &'static str,
}

/// Built-in model roster.
///
/// URLs point to publicly hosted copies of each file.  Update these when model
/// artifacts are published to a new location (GitHub Releases, HuggingFace,
/// etc.).
pub const DEFAULT_MODELS: &[ModelDef] = &[ModelDef {
    id: "clip-vit-b32",
    name: "CLIP ViT-B/32",
    description: "Image categorization via CLIP embeddings (Tier 2 classifier)",
    files: &[ModelFileDef {
        filename: "clip_image.onnx",
        // CLIP ViT-B/32 visual encoder exported to ONNX (Xenova's HuggingFace export).
        url: "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/vision_model.onnx",
    }],
    // The category prototype vectors (clip_prototypes.bin) are embedded in the
    // binary at compile time — no separate download needed.
    size_label: "~340 MB",
}];

/// Resolve the effective models directory from an optional custom path.
pub fn resolve_models_dir(custom_path: &str) -> PathBuf {
    if custom_path.is_empty() {
        crate::model_paths::default_models_dir()
    } else {
        PathBuf::from(custom_path)
    }
}

/// Check whether every file for `model` exists in `models_dir`.
pub fn model_installed(model: &ModelDef, models_dir: &Path) -> bool {
    model
        .files
        .iter()
        .all(|f| models_dir.join(f.filename).is_file())
}

/// Whether every file in `model` has a non-empty download URL.
pub fn model_downloadable(model: &ModelDef) -> bool {
    model.files.iter().all(|f| !f.url.is_empty())
}

/// Look up a model definition by ID.
pub fn find_model(id: &str) -> Option<&'static ModelDef> {
    DEFAULT_MODELS.iter().find(|m| m.id == id)
}

/// Delete all files belonging to `model` from `models_dir`.
pub fn remove_model(model: &ModelDef, models_dir: &Path) -> Result<(), String> {
    for file in model.files {
        let path = models_dir.join(file.filename);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to remove {}: {}", file.filename, e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_models_is_not_empty() {
        assert!(!DEFAULT_MODELS.is_empty());
    }

    #[test]
    fn each_model_has_files() {
        for m in DEFAULT_MODELS {
            assert!(!m.files.is_empty(), "model {} has no files", m.id);
        }
    }

    #[test]
    fn find_model_returns_known() {
        assert!(find_model("clip-vit-b32").is_some());
    }

    #[test]
    fn find_model_returns_none_for_unknown() {
        assert!(find_model("no-such-model").is_none());
    }

    #[test]
    fn model_not_installed_in_nonexistent_dir() {
        let m = &DEFAULT_MODELS[0];
        assert!(!model_installed(m, Path::new("/no/such/dir")));
    }

    #[test]
    fn resolve_models_dir_custom_path() {
        let dir = resolve_models_dir("/custom/path");
        assert_eq!(dir, PathBuf::from("/custom/path"));
    }

    #[test]
    fn resolve_models_dir_empty_uses_default() {
        let dir = resolve_models_dir("");
        assert!(dir.ends_with("shingan/models"));
    }
}
