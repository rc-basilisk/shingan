//! # shingan-core
//!
//! Core detection engine for shingan (心眼), a multi-modal duplicate file detector.
//!
//! This crate provides the [`detector::Detector`] trait as its central abstraction.
//! Each detector implements content-aware similarity comparison for a specific file
//! category:
//!
//! - **Image** -- multi-hash perceptual hashing (aHash + pHash + dHash) with a
//!   10 000-entry parse cache for fast pairwise comparison
//! - **Video** -- 3-D DCT fingerprinting across sampled frames (configurable skip
//!   and duration via [`detector::video::VideoDetector::with_sampling`]) with a
//!   2 000-entry parse cache
//! - **Document** -- text extraction followed by Sorensen-Dice coefficient comparison
//! - **Code** -- whitespace/comment normalization with fuzzy matching
//! - **Archive** -- byte-level SHA-256 for exact-match deduplication
//!
//! All detector caches use `parking_lot::Mutex` for non-poisoning, low-contention
//! locking.
//!
//! ## Scanning pipeline
//!
//! The [`scanner::duplicate::DuplicateScanner`] orchestrates the full scanning
//! pipeline in three phases:
//!
//! 1. **Discovery** -- walk the requested directories via [`scanner::FileScanner`],
//!    classify files by category, apply configurable size limits
//!    ([`scanner::FileScanner::with_size_limits`]), and track permission-denied /
//!    I/O errors in [`scanner::ScanResult`].
//! 2. **Fingerprinting** -- compute a signature for every discovered file using the
//!    appropriate detector (parallelized with rayon). Pre-cached signatures from a
//!    previous scan can be supplied via
//!    [`scanner::duplicate::DuplicateScanner::with_cached_signatures`] to skip
//!    recomputation for unchanged files.
//! 3. **Grouping** -- cluster files whose similarity exceeds the configured threshold
//!    using LSH prefix bucketing and a union-find structure with strict cross-validation
//!    (see [`scanner::grouping`]).
//!
//! [`scanner::duplicate::DuplicateScanner::scan_paths`] returns both the duplicate
//! groups and a list of newly computed `(path, signature)` pairs that the caller
//! should persist (e.g. to the `signature_cache` table in `shingan-db`) so that
//! future rescans benefit from the cache.
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
//! let (results, new_sigs) = scanner.scan_paths(&[("./my_files".into(), true)]);
//! // Persist new_sigs to the database for future rescans.
//! ```

pub mod cache;
pub mod detector;
pub mod enrichment;
pub mod error;
pub mod file_info;
pub mod scanner;
