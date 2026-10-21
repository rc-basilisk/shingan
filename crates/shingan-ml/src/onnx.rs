//! Tier 2: local CLIP-based zero-shot classification via ONNX Runtime.
//!
//! This tier runs an OpenAI CLIP ViT-B/32 image encoder through ONNX Runtime,
//! then compares the resulting embedding against precomputed L2-normalized text
//! prototype vectors (one per [`ImageSubCategory`]) using cosine similarity.
//! Cosines are scaled by CLIP's logit scale (100) and passed through a softmax,
//! giving a probability per category that is meaningful to threshold.
//!
//! One model file is required:
//!
//! - `clip_image.onnx` — the CLIP image encoder exported to ONNX format
//!
//! The category prototype vectors (`clip_prototypes.bin`) are embedded in the
//! binary at compile time.  An on-disk file in the model directory is used if
//! present (allows experimentation), otherwise the built-in copy is used.
//!
//! ONNX Runtime itself is loaded dynamically at runtime. [`ensure_runtime`]
//! looks for the shared library in `ORT_DYLIB_PATH`, next to the executable
//! (and in its `lib/` folder), in the models directory, and finally in the
//! system library path. If it can't be found, Tier 2 is disabled with an error
//! message instead of crashing.
//!
//! When the `onnx` feature is disabled, [`ClipOnnxClassifier`] becomes a
//! zero-sized stub whose `try_load` always returns `Ok(None)`.
//!
//! Image preprocessing follows the standard CLIP recipe: resize the shortest
//! side to 224 (bicubic), center-crop 224×224, scale to `[0, 1]`, and normalize
//! per-channel with the CLIP mean/std constants.

#[cfg(feature = "onnx")]
use crate::model_paths::EXPECTED_PROTOTYPE_ROWS;
#[cfg(feature = "onnx")]
use crate::taxonomy::ImageSubCategory;
#[cfg(feature = "onnx")]
use std::path::{Path, PathBuf};

#[cfg(feature = "onnx")]
use image::imageops::FilterType;
#[cfg(feature = "onnx")]
use ndarray::{Array2, Array4};
#[cfg(feature = "onnx")]
use ort::session::Session;
#[cfg(feature = "onnx")]
use ort::value::TensorRef;

/// OpenAI CLIP ViT-B/32 normalization constants.
#[cfg(feature = "onnx")]
const CLIP_MEAN: [f32; 3] = [0.48145466, 0.45684515, 0.40821073];
#[cfg(feature = "onnx")]
const CLIP_STD: [f32; 3] = [0.26862954, 0.261_302_6, 0.275_777_1];
/// CLIP's learned temperature (logit scale), fixed at 100 for ViT-B/32.
#[cfg(feature = "onnx")]
const LOGIT_SCALE: f32 = 100.0;
#[cfg(feature = "onnx")]
const INPUT_SIZE: u32 = 224;

/// Precomputed L2-normalised CLIP text embeddings for 24 categories (512-dim, LE f32).
#[cfg(feature = "onnx")]
const EMBEDDED_PROTOTYPES: &[u8] = include_bytes!("../data/clip_prototypes.bin");

/// File name prefix of the ONNX Runtime shared library on this platform.
#[cfg(all(feature = "onnx", target_os = "windows"))]
const ORT_LIB_NAMES: &[&str] = &["onnxruntime.dll"];
#[cfg(all(feature = "onnx", target_os = "macos"))]
const ORT_LIB_NAMES: &[&str] = &["libonnxruntime.dylib", "libonnxruntime."];
#[cfg(all(feature = "onnx", not(any(target_os = "windows", target_os = "macos"))))]
const ORT_LIB_NAMES: &[&str] = &["libonnxruntime.so"];

/// Candidate locations of the ONNX Runtime library, in priority order.
#[cfg(feature = "onnx")]
fn runtime_candidates(model_dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
            dirs.push(dir.join("lib"));
            // macOS app bundles / Homebrew-style layouts
            dirs.push(dir.join("../lib"));
        }
    }
    dirs.push(model_dir.to_path_buf());

    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut matches: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                ORT_LIB_NAMES.iter().any(|prefix| name.starts_with(prefix))
                    && !name.ends_with(".pdb")
                    && !name.ends_with(".lib")
            })
            .collect();
        // Prefer the plain name, then shortest versioned name (e.g. .so.1 over .so.1.21.0).
        matches.sort_by_key(|p| p.as_os_str().len());
        found.extend(matches);
    }
    found
}

/// Oldest ONNX Runtime minor version (1.x) this build supports.
#[cfg(feature = "onnx")]
const MIN_ORT_MINOR: u32 = 18;

/// Read the version string of an ONNX Runtime library without involving `ort`.
///
/// `ort` deadlocks if asked to load a runtime that is too old (it builds its
/// error message with the very API it failed to initialize), so incompatible
/// libraries must be rejected before they reach it.
#[cfg(feature = "onnx")]
fn probe_runtime_version(path: &Path) -> Result<String, String> {
    type GetApiBase = unsafe extern "C" fn() -> *const ort::sys::OrtApiBase;
    // SAFETY: loading ONNX Runtime runs no untrusted initialization beyond the
    // library's own constructors; we only call its documented C entry point.
    unsafe {
        let lib = libloading::Library::new(path).map_err(|e| e.to_string())?;
        let get_base: libloading::Symbol<GetApiBase> = lib
            .get(b"OrtGetApiBase")
            .map_err(|_| "not an ONNX Runtime library (no OrtGetApiBase)".to_string())?;
        let base = get_base();
        if base.is_null() {
            return Err("OrtGetApiBase returned null".into());
        }
        let version_ptr = ((*base).GetVersionString)();
        let version = std::ffi::CStr::from_ptr(version_ptr)
            .to_string_lossy()
            .into_owned();
        // Keep the library loaded; `ort` will open it again (refcounted).
        std::mem::forget(lib);
        Ok(version)
    }
}

#[cfg(feature = "onnx")]
fn check_version(version: &str) -> Result<(), String> {
    let minor: u32 = version
        .split('.')
        .nth(1)
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    if minor < MIN_ORT_MINOR {
        Err(format!(
            "version {version} is too old (need 1.{MIN_ORT_MINOR} or newer)"
        ))
    } else {
        Ok(())
    }
}

#[cfg(feature = "onnx")]
static RUNTIME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
#[cfg(feature = "onnx")]
static RUNTIME_ATTEMPT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Load the ONNX Runtime shared library. Returns where it was loaded from, or a
/// human-readable error. Never panics. Success is cached; failures are retried
/// on the next call (so installing the runtime mid-session works).
#[cfg(feature = "onnx")]
pub fn ensure_runtime(model_dir: &Path) -> Result<String, String> {
    if let Some(loc) = RUNTIME.get() {
        return Ok(loc.clone());
    }
    let _guard = RUNTIME_ATTEMPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(loc) = RUNTIME.get() {
        return Ok(loc.clone());
    }
    let loc = load_runtime(model_dir)?;
    let _ = RUNTIME.set(loc.clone());
    Ok(loc)
}

#[cfg(feature = "onnx")]
fn load_runtime(model_dir: &Path) -> Result<String, String> {
    {
        {
            let mut errors = Vec::new();
            let mut candidates: Vec<PathBuf> = Vec::new();
            if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
                if !p.is_empty() {
                    candidates.push(PathBuf::from(p));
                }
            }
            candidates.extend(runtime_candidates(model_dir));

            // Last resort: let the OS loader search its default paths.
            candidates.push(PathBuf::from(ORT_LIB_NAMES[0]));

            for path in &candidates {
                let version = match probe_runtime_version(path) {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(format!("{}: {e}", path.display()));
                        continue;
                    }
                };
                if let Err(e) = check_version(&version) {
                    errors.push(format!("{}: {e}", path.display()));
                    continue;
                }
                match std::panic::catch_unwind(|| ort::init_from(path)) {
                    Ok(Ok(builder)) => {
                        builder.commit();
                        // `ort` releases its global environment from an exit hook,
                        // which runs after a dlopen'ed ONNX Runtime has already torn
                        // down its C++ statics, and segfaults. Leaking one reference
                        // keeps that hook from ever calling `ReleaseEnv`; the OS
                        // reclaims everything at exit anyway.
                        if let Ok(env) = ort::environment::current() {
                            std::mem::forget(env);
                        }
                        return Ok(format!("{} (v{version})", path.display()));
                    }
                    Ok(Err(e)) => errors.push(format!("{}: {e}", path.display())),
                    Err(_) => errors.push(format!("{}: failed to initialize", path.display())),
                }
            }
            Err(runtime_error(&errors))
        }
    }
}

#[cfg(feature = "onnx")]
fn runtime_error(errors: &[String]) -> String {
    let mut msg = String::from(
        "ONNX Runtime library not found. Downloading the model (Settings, or \
         `shingan model download`) also fetches it; or install ONNX Runtime 1.18+ \
         and set ORT_DYLIB_PATH.",
    );
    if !errors.is_empty() {
        msg.push_str(" Tried: ");
        msg.push_str(&errors.join("; "));
    }
    msg
}

/// Loads when `clip_image.onnx` exists and ONNX Runtime is available.
#[cfg(feature = "onnx")]
pub struct ClipOnnxClassifier {
    session: Session,
    /// Row-normalized prototypes, `[categories, embed_dim]`.
    prototypes: Array2<f32>,
    embed_dim: usize,
}

#[cfg(feature = "onnx")]
impl ClipOnnxClassifier {
    /// Try load from `model_dir` (defaults to [`crate::model_paths::default_models_dir`]).
    ///
    /// Returns `Ok(None)` when the model file is simply not installed, and
    /// `Err` when it is installed but cannot be used (missing runtime,
    /// corrupt model, ...).
    pub fn try_load(model_dir: Option<&Path>) -> Result<Option<Self>, String> {
        let default_dir = crate::model_paths::default_models_dir();
        let dir = model_dir.unwrap_or(default_dir.as_path());
        let onnx_path = dir.join("clip_image.onnx");
        if !onnx_path.is_file() {
            return Ok(None);
        }

        let proto_path = dir.join("clip_prototypes.bin");
        let disk_protos;
        let proto_bytes: &[u8] = if proto_path.is_file() {
            disk_protos = std::fs::read(&proto_path).map_err(|e| e.to_string())?;
            &disk_protos
        } else {
            EMBEDDED_PROTOTYPES
        };
        let (prototypes, embed_dim) = parse_prototypes(proto_bytes)?;

        ensure_runtime(dir)?;

        let session = Session::builder()
            .map_err(|e| e.to_string())?
            .commit_from_file(&onnx_path)
            .map_err(|e| format!("failed to load {}: {e}", onnx_path.display()))?;

        Ok(Some(Self {
            session,
            prototypes,
            embed_dim,
        }))
    }

    fn preprocess_image(path: &Path) -> Result<Array4<f32>, String> {
        let img = crate::open_image(path)?.to_rgb8();
        Ok(preprocess_rgb(&img))
    }

    /// Softmax probability for every category, in [`ImageSubCategory::ALL`] order.
    pub fn probabilities(&mut self, path: &Path) -> Result<Vec<f32>, String> {
        let input = Self::preprocess_image(path)?;
        let input_ref = TensorRef::from_array_view(&input).map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs![input_ref])
            .map_err(|e| e.to_string())?;

        let (_shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;

        if data.len() != self.embed_dim {
            return Err(format!(
                "embedding length {} != expected {}",
                data.len(),
                self.embed_dim
            ));
        }

        let norm = data.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        let cosines: Vec<f32> = self
            .prototypes
            .rows()
            .into_iter()
            .map(|row| row.iter().zip(data.iter()).map(|(p, e)| p * e).sum::<f32>() / norm)
            .collect();
        Ok(softmax(&cosines, LOGIT_SCALE))
    }

    /// Returns the best category and its probability.
    pub fn classify(&mut self, path: &Path) -> Result<(ImageSubCategory, f32), String> {
        let probs = self.probabilities(path)?;
        let (i, p) = argmax(&probs);
        Ok((ImageSubCategory::ALL[i], p))
    }
}

/// Parse and L2-normalize the prototype matrix.
#[cfg(feature = "onnx")]
fn parse_prototypes(bytes: &[u8]) -> Result<(Array2<f32>, usize), String> {
    if !bytes.len().is_multiple_of(4) {
        return Err("invalid prototype file size".into());
    }
    let n_floats = bytes.len() / 4;
    if n_floats == 0 || !n_floats.is_multiple_of(EXPECTED_PROTOTYPE_ROWS) {
        return Err("prototype rows must match taxonomy".into());
    }
    let embed_dim = n_floats / EXPECTED_PROTOTYPE_ROWS;
    let floats: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let mut protos = Array2::from_shape_vec((EXPECTED_PROTOTYPE_ROWS, embed_dim), floats)
        .map_err(|e| e.to_string())?;
    for mut row in protos.rows_mut() {
        let n = row.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        row.mapv_inplace(|x| x / n);
    }
    Ok((protos, embed_dim))
}

/// CLIP preprocessing: shortest side -> 224 (bicubic), center crop, normalize. NCHW.
#[cfg(feature = "onnx")]
pub(crate) fn preprocess_rgb(img: &image::RgbImage) -> Array4<f32> {
    let (w, h) = img.dimensions();
    let scale = INPUT_SIZE as f32 / w.min(h).max(1) as f32;
    let nw = ((w as f32 * scale).round() as u32).max(INPUT_SIZE);
    let nh = ((h as f32 * scale).round() as u32).max(INPUT_SIZE);
    let resized = image::imageops::resize(img, nw, nh, FilterType::CatmullRom);
    let x0 = (nw - INPUT_SIZE) / 2;
    let y0 = (nh - INPUT_SIZE) / 2;

    let s = INPUT_SIZE as usize;
    let mut arr = Array4::<f32>::zeros((1, 3, s, s));
    for y in 0..INPUT_SIZE {
        for x in 0..INPUT_SIZE {
            let p = resized.get_pixel(x0 + x, y0 + y);
            for c in 0..3 {
                arr[[0, c, y as usize, x as usize]] =
                    (p[c] as f32 / 255.0 - CLIP_MEAN[c]) / CLIP_STD[c];
            }
        }
    }
    arr
}

/// Numerically stable softmax of `scale * values`.
#[cfg(feature = "onnx")]
pub(crate) fn softmax(values: &[f32], scale: f32) -> Vec<f32> {
    let max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = values.iter().map(|v| ((v - max) * scale).exp()).collect();
    let sum: f32 = exps.iter().sum::<f32>().max(1e-12);
    exps.iter().map(|e| e / sum).collect()
}

/// Index and value of the largest element (first on ties).
#[cfg(feature = "onnx")]
pub(crate) fn argmax(values: &[f32]) -> (usize, f32) {
    values
        .iter()
        .copied()
        .enumerate()
        .fold((0, f32::NEG_INFINITY), |best, (i, v)| {
            if v > best.1 {
                (i, v)
            } else {
                best
            }
        })
}

/// Stub when the `onnx` feature is disabled — always returns `None` from [`ClipOnnxClassifier::try_load`].
#[cfg(not(feature = "onnx"))]
pub struct ClipOnnxClassifier;

#[cfg(not(feature = "onnx"))]
impl ClipOnnxClassifier {
    /// Returns `Ok(None)` since ONNX support is not compiled in.
    pub fn try_load(_model_dir: Option<&std::path::Path>) -> Result<Option<Self>, String> {
        Ok(None)
    }
}

#[cfg(all(test, feature = "onnx"))]
mod tests {
    use super::*;

    #[test]
    fn softmax_sums_to_one_and_is_peaked() {
        let p = softmax(&[0.30, 0.25, 0.20], LOGIT_SCALE);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(p[0] > 0.99);
        assert_eq!(argmax(&p).0, 0);
    }

    #[test]
    fn embedded_prototypes_parse_and_are_normalized() {
        let (protos, dim) = parse_prototypes(EMBEDDED_PROTOTYPES).unwrap();
        assert_eq!(dim, 512);
        for row in protos.rows() {
            let n: f32 = row.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((n - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn preprocess_center_crops_wide_images() {
        // Left third red, middle third green, right third blue.
        let img = image::RgbImage::from_fn(672, 224, |x, _| match x / 224 {
            0 => image::Rgb([255, 0, 0]),
            1 => image::Rgb([0, 255, 0]),
            _ => image::Rgb([0, 0, 255]),
        });
        let arr = preprocess_rgb(&img);
        assert_eq!(arr.shape(), &[1, 3, 224, 224]);
        // Center pixel comes from the green third.
        let g = arr[[0, 1, 112, 112]];
        let r = arr[[0, 0, 112, 112]];
        assert!(g > 1.0 && r < 0.0, "g={g} r={r}");
    }

    #[test]
    fn version_check() {
        assert!(check_version("1.21.0").is_ok());
        assert!(check_version("1.18.1").is_ok());
        assert!(check_version("1.17.0").is_err());
        assert!(check_version("garbage").is_err());
    }

    #[test]
    fn missing_model_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ClipOnnxClassifier::try_load(Some(dir.path()))
            .unwrap()
            .is_none());
    }
}
