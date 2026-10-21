//! # shingan-core
//!
//! Core detection engine for shingan (心眼), a multi-modal duplicate file detector.
//!
//! This crate provides the [`detector::Detector`] trait as its central abstraction.
//! Each detector implements content-aware similarity comparison for a specific file
//! category:
//!
//! - **Image** -- multi-hash perceptual hashing (aHash + pHash + dHash); all three
//!   must agree
//! - **Video** -- 3-D DCT fingerprinting across sampled frames (configurable skip
//!   and duration via [`detector::video::VideoDetector::with_sampling`])
//! - **Document** -- text extraction followed by MinHash over word shingles
//!   (Sorensen-Dice similarity)
//! - **Code** -- comment/whitespace normalization, tokenization and MinHash
//! - **Archive** -- byte-level SHA-256 for exact-match deduplication
//!
//! Detectors expose locality-sensitive bucket keys
//! ([`detector::Detector::candidate_keys`]) so the grouping phase compares only
//! plausible pairs instead of every pair.
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
//!    appropriate detector (parallelized with rayon). A persistent
//!    [`scanner::duplicate::SignatureCache`] can be attached via
//!    [`scanner::duplicate::DuplicateScanner::with_signature_cache`] to skip
//!    recomputation for unchanged files.
//! 3. **Grouping** -- cluster files whose similarity exceeds the configured threshold
//!    using LSH bucketing and a union-find structure with strict cross-validation
//!    (see [`scanner::grouping`]).
//!
//! [`scanner::duplicate::DuplicateScanner::scan_paths`] returns both the duplicate
//! groups and the newly computed signatures, which the caller should persist
//! (e.g. to the `signature_cache` table in `shingan-db`) so that future rescans
//! benefit from the cache.
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

pub mod detector;
pub mod enrichment;
pub mod error;
pub mod file_info;
pub mod scanner;
