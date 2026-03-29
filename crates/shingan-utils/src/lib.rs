//! # shingan-utils
//!
//! Utility modules for the shingan workspace.
//!
//! - [`auto_sorter::AutoSorter`] -- organizes files into category-based directory
//!   structures (e.g. images, documents, code) using file-extension heuristics.
//! - [`ml_categorizer`] -- optional ML-powered file categorization via a local
//!   [Ollama](https://ollama.com) instance, for cases where extension-based
//!   classification is insufficient.

pub mod auto_sorter;
pub mod ml_categorizer;
