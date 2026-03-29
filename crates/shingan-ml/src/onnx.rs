//! Tier 2: local CLIP-based zero-shot classification via ONNX Runtime.
//!
//! This tier runs an OpenAI CLIP ViT-B/32 image encoder through ONNX Runtime,
//! then compares the resulting embedding against precomputed L2-normalized text
//! prototype vectors (one per [`ImageSubCategory`]) using cosine similarity.
//! The category with the highest similarity score is returned.
//!
//! Two model files are required (see [`crate::model_paths`]):
//!
//! - `clip_image.onnx` — the CLIP image encoder exported to ONNX format
//! - `clip_prototypes.bin` — a flat binary file of `24 × embed_dim` float32
//!   values (little-endian), where each row is the L2-normalized CLIP text
//!   embedding for the corresponding [`ImageSubCategory::clip_prompt`]
//!
//! When the `onnx` feature is disabled, [`ClipOnnxClassifier`] becomes a
//! zero-sized stub whose `try_load` always returns `Ok(None)`.
//!
//! Image preprocessing follows the standard CLIP normalization: resize to
//! 224×224, scale to `[0, 1]`, and normalize per-channel with the CLIP
//! mean/std constants.

#[cfg(feature = "onnx")]
use crate::model_paths::EXPECTED_PROTOTYPE_ROWS;
#[cfg(feature = "onnx")]
use crate::taxonomy::ImageSubCategory;
#[cfg(feature = "onnx")]
use std::path::Path;

#[cfg(feature = "onnx")]
use image::imageops::FilterType;
#[cfg(feature = "onnx")]
use ndarray::{Array1, Array2, Array4};
#[cfg(feature = "onnx")]
use ort::session::Session;
#[cfg(feature = "onnx")]
use ort::value::TensorRef;

/// OpenAI CLIP ViT-B/32 normalization constants.
#[cfg(feature = "onnx")]
const CLIP_MEAN: [f32; 3] = [0.48145466, 0.45684515, 0.40821073];
#[cfg(feature = "onnx")]
const CLIP_STD: [f32; 3] = [0.26862954, 0.261_302_6, 0.275_777_1];

/// Loads when `clip_image.onnx` and `clip_prototypes.bin` exist and are consistent.
#[cfg(feature = "onnx")]
pub struct ClipOnnxClassifier {
    session: Session,
    prototypes: Array2<f32>,
    embed_dim: usize,
}

#[cfg(feature = "onnx")]
impl ClipOnnxClassifier {
    /// Try load from `model_dir` (defaults to [`crate::model_paths::default_models_dir`]).
    pub fn try_load(model_dir: Option<&Path>) -> Result<Option<Self>, String> {
        let default_dir = crate::model_paths::default_models_dir();
        let dir = model_dir.unwrap_or(default_dir.as_path());
        let onnx_path = dir.join("clip_image.onnx");
        let proto_path = dir.join("clip_prototypes.bin");
        if !onnx_path.is_file() || !proto_path.is_file() {
            return Ok(None);
        }
        let proto_bytes = std::fs::read(&proto_path).map_err(|e| e.to_string())?;
        if proto_bytes.len() % 4 != 0 {
            return Err("invalid prototype file size".into());
        }
        let n_floats = proto_bytes.len() / 4;
        if n_floats % EXPECTED_PROTOTYPE_ROWS != 0 {
            return Err("prototype rows must match taxonomy".into());
        }
        let embed_dim = n_floats / EXPECTED_PROTOTYPE_ROWS;
        let floats: Vec<f32> = proto_bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let prototypes =
            Array2::from_shape_vec((EXPECTED_PROTOTYPE_ROWS, embed_dim), floats)
                .map_err(|e| e.to_string())?;

        let session = Session::builder()
            .map_err(|e| e.to_string())?
            .commit_from_file(onnx_path)
            .map_err(|e| e.to_string())?;

        Ok(Some(Self {
            session,
            prototypes,
            embed_dim,
        }))
    }

    fn preprocess_image(path: &Path) -> Result<Array4<f32>, String> {
        let img = image::open(path).map_err(|e| e.to_string())?.to_rgb8();
        let resized = image::imageops::resize(&img, 224, 224, FilterType::Triangle);
        let mut arr = Array4::<f32>::zeros((1, 3, 224, 224));
        for (x, y, p) in resized.enumerate_pixels() {
            arr[[0, 0, y as usize, x as usize]] = (p[0] as f32 / 255.0 - CLIP_MEAN[0]) / CLIP_STD[0];
            arr[[0, 1, y as usize, x as usize]] = (p[1] as f32 / 255.0 - CLIP_MEAN[1]) / CLIP_STD[1];
            arr[[0, 2, y as usize, x as usize]] = (p[2] as f32 / 255.0 - CLIP_MEAN[2]) / CLIP_STD[2];
        }
        Ok(arr)
    }

    fn l2_normalize(v: &Array1<f32>) -> Array1<f32> {
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        v / norm
    }

    /// Returns best category and cosine similarity (confidence proxy).
    pub fn classify(&mut self, path: &Path) -> Result<(ImageSubCategory, f32), String> {
        let input = Self::preprocess_image(path)?;
        let input_ref = TensorRef::from_array_view(&input).map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs![input_ref])
            .map_err(|e| e.to_string())?;

        let output = &outputs[0];
        let (_shape, data) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;

        if data.len() != self.embed_dim {
            return Err(format!(
                "embedding length {} != expected {}",
                data.len(),
                self.embed_dim
            ));
        }

        let emb = Array1::from_vec(data.to_vec());
        let emb_n = Self::l2_normalize(&emb);
        let mut best_i = 0usize;
        let mut best_s = -1.0f32;
        for i in 0..EXPECTED_PROTOTYPE_ROWS {
            let row = self.prototypes.row(i).to_owned();
            let p_n = Self::l2_normalize(&row);
            let s = emb_n.dot(&p_n);
            if s > best_s {
                best_s = s;
                best_i = i;
            }
        }
        let cat = ImageSubCategory::ALL[best_i];
        Ok((cat, best_s.clamp(0.0, 1.0)))
    }
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
