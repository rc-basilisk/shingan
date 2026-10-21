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
    description: "Recognizes photos, screenshots, art, memes, documents and more (runs offline)",
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
    if custom_path.trim().is_empty() {
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

/// Temporary name used while a file is downloading. Only renamed to the final
/// name once complete, so an interrupted download never looks "installed".
pub fn partial_path(final_path: &Path) -> PathBuf {
    let mut name = final_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    final_path.with_file_name(name)
}

/// Download every file of `model` into `models_dir` (blocking).
///
/// `progress` receives `(file_name, downloaded_bytes, total_bytes)`; total is 0
/// when the server doesn't report a length.
#[cfg(feature = "cloud")]
pub fn download_model_blocking(
    model: &ModelDef,
    models_dir: &Path,
    progress: &dyn Fn(&str, u64, u64),
) -> Result<(), String> {
    use std::io::{Read, Write};

    std::fs::create_dir_all(models_dir).map_err(|e| e.to_string())?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(None)
        .build()
        .map_err(|e| e.to_string())?;

    for file in model.files {
        if file.url.is_empty() {
            return Err(format!("no download URL configured for {}", file.filename));
        }
        let target = models_dir.join(file.filename);
        let part = partial_path(&target);
        let mut resp = client
            .get(file.url)
            .send()
            .map_err(|e| format!("{}: {e}", file.filename))?;
        if !resp.status().is_success() {
            return Err(format!("{}: HTTP {}", file.filename, resp.status()));
        }
        let total = resp.content_length().unwrap_or(0);
        let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        loop {
            let n = resp
                .read(&mut buf)
                .map_err(|e| format!("{}: {e}", file.filename))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            progress(file.filename, done, total);
        }
        out.flush().map_err(|e| e.to_string())?;
        drop(out);
        if total > 0 && done != total {
            let _ = std::fs::remove_file(&part);
            return Err(format!(
                "{}: incomplete download ({done} of {total} bytes)",
                file.filename
            ));
        }
        std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
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
    fn partial_path_appends_suffix() {
        let p = partial_path(Path::new("/m/clip_image.onnx"));
        assert_eq!(p, PathBuf::from("/m/clip_image.onnx.part"));
    }

    #[test]
    fn resolve_models_dir_empty_uses_default() {
        let dir = resolve_models_dir("");
        assert!(dir.ends_with("shingan/models"));
    }
}
