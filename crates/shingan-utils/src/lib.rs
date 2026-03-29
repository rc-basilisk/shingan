//! # shingan-utils
//!
//! Utility modules for the shingan workspace.
//!
//! - [`auto_sorter::AutoSorter`] -- organizes files into category-based directory
//!   structures (e.g. images, documents, code) using file-extension heuristics.
//!   When ML-based sub-categorization is enabled, the sorter delegates to
//!   [`shingan_ml::TieredPipeline`] for local image classification.

pub mod auto_sorter;
