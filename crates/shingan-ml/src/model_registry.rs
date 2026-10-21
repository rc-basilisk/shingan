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

/// ONNX Runtime release fetched alongside the model when no runtime is installed.
pub const ORT_VERSION: &str = "1.22.0";

/// Where to get the ONNX Runtime shared library for this platform.
#[derive(Debug, Clone)]
pub struct RuntimeAsset {
    pub url: String,
    /// Path suffix of the library inside the downloaded archive.
    pub member_suffix: String,
    /// File name to save it under (what the loader looks for).
    pub file_name: &'static str,
    pub is_zip: bool,
}

/// The ONNX Runtime download for the current OS/architecture, if one exists.
pub fn runtime_asset() -> Option<RuntimeAsset> {
    let v = ORT_VERSION;
    let (platform, member, file_name, is_zip) =
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            (
                "linux-x64",
                format!("lib/libonnxruntime.so.{v}"),
                "libonnxruntime.so",
                false,
            )
        } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            (
                "linux-aarch64",
                format!("lib/libonnxruntime.so.{v}"),
                "libonnxruntime.so",
                false,
            )
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            (
                "osx-arm64",
                format!("lib/libonnxruntime.{v}.dylib"),
                "libonnxruntime.dylib",
                false,
            )
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            (
                "osx-x86_64",
                format!("lib/libonnxruntime.{v}.dylib"),
                "libonnxruntime.dylib",
                false,
            )
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            (
                "win-x64",
                "lib/onnxruntime.dll".to_string(),
                "onnxruntime.dll",
                true,
            )
        } else {
            return None;
        };
    let ext = if is_zip { "zip" } else { "tgz" };
    Some(RuntimeAsset {
        url: format!(
            "https://github.com/microsoft/onnxruntime/releases/download/v{v}/onnxruntime-{platform}-{v}.{ext}"
        ),
        member_suffix: member,
        file_name,
        is_zip,
    })
}

/// Whether a downloaded ONNX Runtime library is present in `models_dir`.
pub fn runtime_downloaded(models_dir: &Path) -> bool {
    runtime_asset().is_some_and(|a| models_dir.join(a.file_name).is_file())
}

/// Progress callback: `(item name, item index, item count, downloaded bytes, total bytes)`.
pub type DownloadProgress<'a> = &'a dyn Fn(&str, usize, usize, u64, u64);

#[cfg(feature = "cloud")]
fn fetch(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    (label, index, count): (&str, usize, usize),
    progress: DownloadProgress<'_>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    use std::io::{Read, Write};
    use std::sync::atomic::Ordering;

    let part = partial_path(dest);
    let result = (|| {
        let mut resp = client
            .get(url)
            .send()
            .map_err(|e| format!("{label}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("{label}: HTTP {}", resp.status()));
        }
        let total = resp.content_length().unwrap_or(0);
        let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".to_string());
            }
            let n = resp.read(&mut buf).map_err(|e| format!("{label}: {e}"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            progress(label, index, count, done, total);
        }
        out.flush().map_err(|e| e.to_string())?;
        if total > 0 && done != total {
            return Err(format!(
                "{label}: incomplete download ({done} of {total} bytes)"
            ));
        }
        Ok(())
    })();
    match result {
        Ok(()) => std::fs::rename(&part, dest).map_err(|e| e.to_string()),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// Pull one file out of a `.tgz` or `.zip` archive.
#[cfg(feature = "cloud")]
fn extract_member(archive: &Path, suffix: &str, is_zip: bool, dest: &Path) -> Result<(), String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    if is_zip {
        let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let name = (0..zip.len())
            .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|n| n.ends_with(suffix))
            .ok_or_else(|| format!("{suffix} not found in archive"))?;
        zip.by_name(&name)
            .map_err(|e| e.to_string())?
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
    } else {
        let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        let mut found = false;
        for entry in tar.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry
                .path()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned();
            if path.ends_with(suffix) && entry.header().entry_type().is_file() {
                entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                found = true;
                break;
            }
        }
        if !found {
            return Err(format!("{suffix} not found in archive"));
        }
    }
    let part = partial_path(dest);
    std::fs::write(&part, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

/// Download every file of `model` into `models_dir` (blocking), plus the ONNX
/// Runtime library when `with_runtime` is true and none is there yet.
///
/// Files are written to `<name>.part` and renamed when complete, so an
/// interrupted download never looks installed. Setting `cancel` aborts.
#[cfg(feature = "cloud")]
pub fn download_model_blocking(
    model: &ModelDef,
    models_dir: &Path,
    with_runtime: bool,
    progress: DownloadProgress<'_>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    std::fs::create_dir_all(models_dir).map_err(|e| e.to_string())?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(None)
        .build()
        .map_err(|e| e.to_string())?;

    let runtime = runtime_asset().filter(|_| with_runtime && !runtime_downloaded(models_dir));
    let count = model.files.len() + usize::from(runtime.is_some());

    for (i, file) in model.files.iter().enumerate() {
        let target = models_dir.join(file.filename);
        if target.is_file() {
            continue;
        }
        if file.url.is_empty() {
            return Err(format!("no download URL configured for {}", file.filename));
        }
        fetch(
            &client,
            file.url,
            &target,
            (file.filename, i, count),
            progress,
            cancel,
        )?;
    }

    if let Some(asset) = runtime {
        let label = format!("ONNX Runtime {ORT_VERSION}");
        let archive = models_dir.join(if asset.is_zip {
            "onnxruntime.zip"
        } else {
            "onnxruntime.tgz"
        });
        fetch(
            &client,
            &asset.url,
            &archive,
            (&label, count - 1, count),
            progress,
            cancel,
        )?;
        let result = extract_member(
            &archive,
            &asset.member_suffix,
            asset.is_zip,
            &models_dir.join(asset.file_name),
        );
        let _ = std::fs::remove_file(&archive);
        result?;
    }
    Ok(())
}

/// Delete the downloaded ONNX Runtime library (if any) from `models_dir`.
pub fn remove_runtime(models_dir: &Path) {
    if let Some(a) = runtime_asset() {
        let _ = std::fs::remove_file(models_dir.join(a.file_name));
    }
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
    fn runtime_asset_exists_for_this_platform() {
        if cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        )) {
            let a = runtime_asset().expect("asset");
            assert!(a.url.contains(ORT_VERSION));
            assert!(a
                .url
                .starts_with("https://github.com/microsoft/onnxruntime/"));
        }
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
