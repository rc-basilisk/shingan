//! # shingan-utils
//!
//! Utility modules shared by the shingan CLI and GUI.
//!
//! - [`auto_sorter::AutoSorter`] -- organizes files into category-based directory
//!   structures (e.g. images, documents, code) using file-extension heuristics,
//!   with optional ML image sub-categorization via [`shingan_ml::TieredPipeline`],
//!   dry runs, and undo journals.
//! - [`cleanup`] -- safe removal of duplicates (trash by default, never the last
//!   copy of a group).
//! - [`settings`] -- the persisted user settings (`settings.json`) and helpers
//!   that turn them into ML pipeline/cloud configuration.

pub mod auto_sorter;
pub mod cleanup;
pub mod settings;
