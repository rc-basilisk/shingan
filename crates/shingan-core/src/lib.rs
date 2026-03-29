//! # shingan-core
//!
//! Core detection engine for shingan (心眼), a multi-modal duplicate file detector.
//!
//! This crate provides the [`detector::Detector`] trait as its central abstraction.
//! Each detector implements content-aware similarity comparison for a specific file
//! category:
//!
//! - **Image** -- perceptual hashing (pHash) for visual similarity
//! - **Video** -- 3-D DCT fingerprinting across sampled frames
//! - **Document** -- text extraction followed by Sorensen-Dice coefficient comparison
//! - **Code** -- whitespace/comment normalization with fuzzy matching
//! - **Archive** -- byte-level SHA-256 for exact-match deduplication
//!
//! The [`scanner::duplicate::DuplicateScanner`] orchestrates the full scanning
//! pipeline in three phases:
//!
//! 1. **Discovery** -- walk the requested directories and classify files by category.
//! 2. **Fingerprinting** -- compute a signature for every discovered file using the
//!    appropriate detector.
//! 3. **Comparison** -- cluster files whose similarity exceeds the configured
//!    threshold into duplicate groups.
//!
//! ## Feature flags
//!
//! Individual detectors can be compiled in or out via Cargo features:
//!
//! - `image-detect` -- enables [`detector::image::ImageDetector`]
//! - `document-detect` -- enables [`detector::document::DocumentDetector`]
//! - `code-detect` -- enables [`detector::code::CodeDetector`]
//! - `video-detect` -- enables [`detector::video::VideoDetector`]
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use shingan_core::detector::Detector;
//! use shingan_core::detector::archive::ArchiveDetector;
//! use shingan_core::file_info::FileCategory;
//! use shingan_core::scanner::duplicate::{DuplicateScanner, ScanControl};
//! use std::collections::HashMap;
//! use std::sync::Arc;
//!
//! let threshold = 0.95;
//! let categories = vec![FileCategory::Archive];
//! let mut detectors: HashMap<FileCategory, Box<dyn Detector>> = HashMap::new();
//! detectors.insert(FileCategory::Archive, Box::new(ArchiveDetector::new(threshold)));
//!
//! let (tx, rx) = crossbeam_channel::unbounded();
//! let control = Arc::new(ScanControl::new());
//! let scanner = DuplicateScanner::new(&categories, detectors, threshold, control, tx);
//!
//! let results = scanner.scan_paths(&[("./my_files".into(), true)]);
//! ```

pub mod cache;
pub mod detector;
pub mod error;
pub mod file_info;
pub mod scanner;
