<p align="center">
  <img src="assets/shingan-logo.png" width="200" alt="shingan logo">
</p>

<h1 align="center">shingan</h1>

<p align="center">
  <strong>Seeing through files to find the hidden doubles.</strong><br>
  <em>shingan (心眼) — the mind's eye that perceives what lies beneath the surface.</em>
</p>

<p align="center">
  <a href="#features">Features</a> &middot;
  <a href="#installation">Installation</a> &middot;
  <a href="#usage">Usage</a> &middot;
  <a href="#architecture">Architecture</a> &middot;
  <a href="#license">License</a>
</p>

---

shingan is a high-performance file deduplication toolkit written in Rust. It detects duplicate and near-duplicate files across images, videos, documents, code, and archives using perceptual hashing, text similarity, and content-aware analysis. It ships as both a CLI tool and an Iced-based GUI application.

## Features

- **Perceptual duplicate detection** -- finds near-duplicates, not just byte-identical copies
- **3-phase scanning** -- file discovery, parallel signature analysis, union-find fuzzy grouping with strict membership validation
- **5 detection engines** -- image, video, document, code, and archive, each with specialized algorithms
- **Pluggable architecture** -- compile only the detectors you need via feature flags
- **Persistent signature cache** -- computed signatures are stored in SQLite keyed by file path, size, and modification time; rescanning unchanged files skips computation entirely
- **In-memory LRU caches** -- per-detector signature and parse caches (`parking_lot::Mutex`) for fast within-scan comparisons
- **File size filtering** -- configurable min/max size limits with skip-count reporting
- **Pause / resume / stop** -- full scan lifecycle control
- **Progress tracking** -- elapsed time, ETA, and percentage displayed in both CLI and GUI progress bars
- **GUI with preview** -- image thumbnails, syntax-highlighted code, PDF pages, video frames; paginated results (50 groups/page)
- **Auto-sorter** -- rule-based file organization with optional Ollama-powered ML categorization
- **SQLite persistence** -- WAL mode, indexed queries, batch inserts with transactions, full scan history
- **CSV export** -- batch export results for external processing
- **Robust error handling** -- `anyhow`-based CLI with contextual errors; permission-denied and I/O error tracking during scans

## Detection Capabilities

| Category | Algorithm | Details |
|----------|-----------|---------|
| Image | Multi-hash perceptual (aHash + pHash + dHash) | 12x12 bit hashes via `img_hash`; all three must agree; 5000-entry signature cache + 10000-entry parse cache |
| Video | 3D DCT perceptual (`vid_dup_finder_lib` + FFmpeg) | Samples first 20s, skips 3s intro; 1000-entry signature cache + 2000-entry parse cache |
| Document | Text extraction + Sorensen-Dice coefficient | Supports PDF, DOCX, ODT, TXT, SRT, VTT, SUB, RTF |
| Code | Normalization + Sorensen-Dice coefficient | Strips comments and whitespace; syntax-aware comparison |
| Archive | SHA-256 exact match | Byte-for-byte content comparison |

## Supported Formats

| Type | Extensions |
|-----------|------------|
| Images | jpg, jpeg, png, gif, bmp, webp, tiff, svg |
| Documents | txt, doc, docx, odt, pdf, rtf, srt, vtt, sub |
| Videos | mp4, avi, mkv, mov, wmv, flv, webm, m4v |
| Archives | zip, tar, gz, bz2, xz, 7z, rar, zst |
| Code | py, js, ts, exs, html, css, jsx, tsx, vue, rs, go, cpp, c, h |

## Installation

### Prerequisites

- **Rust 1.80+**
- **FFmpeg / ffprobe** on `PATH` (required for video detection)
- **Ollama** running locally (optional, for ML-powered file categorization)

### Build from source

```bash
git clone https://github.com/rc-basilisk/shingan.git
cd shingan
cargo build --release
```

The `shingan` binary will be at `target/release/shingan`.

### Feature flags

All detection engines are enabled by default. Compile with only what you need:

```bash
# All detectors (default)
cargo build --release

# Only image and document detection
cargo build --release --no-default-features --features image-detect,document-detect
```

Available flags: `image-detect`, `document-detect`, `code-detect`, `video-detect`.

## Usage

### CLI

```bash
# Scan directories for image and document duplicates with 95% similarity threshold
shingan scan ~/Photos ~/Documents -t image,document -T 0.95

# List results from previous scans
shingan list
shingan list <SESSION_ID>

# Export a scan session to CSV
shingan export <SESSION_ID> -o results.csv
```

### GUI

```bash
# Launch the graphical interface
cargo run --release -p shingan-gui
```

The GUI provides tabbed access to the duplicate finder, auto-sorter, and settings. It supports dark and light themes, inline file preview, and batch deletion of duplicates.

<!-- TODO: add screenshot -->

## Architecture

shingan is organized as a Cargo workspace with five crates:

| Crate | Role |
|-------|------|
| `shingan-core` | Core detection engine with the pluggable `Detector` trait, LSH grouping, and scan orchestration |
| `shingan-db` | SQLite persistence layer (WAL mode, bundled via `rusqlite`) |
| `shingan-utils` | File sorting utilities and Ollama-based ML image categorization |
| `shingan-cli` | CLI binary (`shingan`) exposing scan, list, and export commands |
| `shingan-gui` | Iced-based GUI with duplicate finder, auto-sorter, and settings tabs |

### Key design decisions

- **Pluggable `Detector` trait** -- each file category implements a common interface for signature computation and comparison, making it straightforward to add new detection strategies.
- **Two-tier LRU caching** -- each detector maintains a bounded signature cache, and image/video detectors add a second parse cache to avoid re-deserializing signatures during pairwise comparisons. All caches use `parking_lot::Mutex` for non-poisoning, low-contention locking.
- **Union-find grouping** -- duplicate candidates are clustered using LSH prefix bucketing, then merged via a union-find structure with path compression and union-by-rank. An incrementally maintained member map avoids O(n) scans per merge. Strict cross-validation before each merge ensures every file in a group is similar to every other file.
- **rayon** -- signature computation is parallelized across available cores.
- **crossbeam-channel** -- progress updates flow from worker threads to the UI without blocking.
- **Persistent signature cache** -- a `signature_cache` table stores computed signatures keyed by `(file_path, file_size, modified_at, category)`. On rescan, unchanged files resolve from cache in microseconds instead of recomputing from disk. Modified files are automatically invalidated and recomputed.
- **Batch DB inserts** -- duplicate groups are persisted in a single SQLite transaction, reducing lock contention and improving write throughput.
- **Indexed queries** -- `file_path` and `created_at` columns are indexed for fast lookups and session listing.

## License

This project is released into the public domain under [CC0 1.0 Universal](LICENSE).

To the extent possible under law, the author has waived all copyright and related or neighboring rights to this work.
