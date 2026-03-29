//! # shingan-ml
//!
//! Multi-tier, local-first image classification for the shingan file organizer.
//!
//! This crate classifies images into one of 24 sub-categories (defined in
//! [`taxonomy::ImageSubCategory`]) using an escalating pipeline that starts with
//! near-zero-cost heuristics and only reaches for heavier inference when
//! confidence is insufficient. The design keeps average classification time low
//! while maintaining accuracy on ambiguous images.
//!
//! ## Pipeline tiers
//!
//! | Tier | Module | Method | Typical latency |
//! |------|--------|--------|-----------------|
//! | 0 | [`tier0`] | Metadata heuristics (resolution, EXIF, aspect ratio) | ~0 ms |
//! | 1 | [`tier1`] | Structural signals (brightness, edge energy, perceptual hash) | ~5 ms |
//! | 2 | [`onnx`] | Local ONNX Runtime CLIP ViT-B/32 image encoder vs. precomputed text prototypes | ~50 ms |
//! | 3 | [`cloud`] | Optional remote vision APIs (Ollama, OpenAI, Gemini) | ~500 ms |
//!
//! The pipeline is orchestrated by [`pipeline::TieredPipeline`]. Each tier has a
//! configurable confidence threshold (see [`pipeline::PipelineConfig`]); if a
//! tier's best result exceeds its threshold the pipeline returns immediately,
//! otherwise it escalates.
//!
//! ## Modules
//!
//! - [`taxonomy`] — the 24-variant `ImageSubCategory` enum with labels, display
//!   names, and CLIP text prompts.
//! - [`tier0`] — fast heuristic classification from `FileInfo` metadata.
//! - [`tier1`] — cheap structural analysis (luma, edges) from pixel sampling.
//! - [`onnx`] — CLIP image embedding + cosine similarity against category
//!   prototypes (requires the `onnx` feature).
//! - [`cloud`] — the `CloudCategorizer` trait and concrete implementations for
//!   Ollama, OpenAI, and Gemini vision APIs.
//! - [`pipeline`] — `TieredPipeline`, `PipelineConfig`, and
//!   `ClassificationResult`.
//! - [`model_paths`] — default filesystem locations for ONNX model files and
//!   prototype vectors, plus validation helpers.
//!
//! ## Feature flags
//!
//! | Flag | Default | Effect |
//! |------|---------|--------|
//! | `onnx` | **yes** | Enables Tier 2 local CLIP inference via `ort` + `ndarray` |
//! | `cloud-ollama` | no | Enables `OllamaCategorizer` (Ollama `/api/generate`) |
//! | `cloud-openai` | no | Enables `OpenAiCategorizer` (stub) |
//! | `cloud-gemini` | no | Enables `GeminiCategorizer` (stub) |
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use shingan_ml::{TieredPipeline, PipelineConfig, ImageSubCategory};
//! use shingan_core::file_info::{FileCategory, FileInfo};
//! use std::path::Path;
//!
//! let mut pipeline = TieredPipeline::new(PipelineConfig::default());
//!
//! let path = Path::new("photo.jpg");
//! let mut info = FileInfo::from_path(path, FileCategory::Image).unwrap();
//! info.enrich_metadata();
//!
//! let result = pipeline.classify_local(path, &info);
//! println!("{:?} (tier {}, confidence {:.2})",
//!     result.category, result.tier, result.confidence);
//! ```

pub mod cloud;
pub mod model_paths;
#[cfg(feature = "onnx")]
pub mod onnx;
pub mod pipeline;
pub mod taxonomy;
pub mod tier0;
pub mod tier1;

pub use cloud::CloudCategorizer;
pub use pipeline::{ClassificationResult, PipelineConfig, TieredPipeline};
pub use taxonomy::ImageSubCategory;
