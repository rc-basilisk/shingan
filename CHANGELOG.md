# Changelog

All notable changes to shingan are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-10-21

A large reliability, safety and usability update. **Upgrading discards cached
fingerprints** (their format changed), so the first scan after upgrading takes
as long as a fresh one. Scan history is kept.

### Highlights

- Near-duplicate **documents and code are actually detected now.** In 0.1.0
  they were fingerprinted with SHA-256, so only exact text matches were found.
  They now use MinHash, reporting Sørensen-Dice similarity.
- **Deletion is safe by default:** files go to the system trash, at least one
  copy of every group is always kept, and both the app and the CLI ask for
  confirmation.
- **Auto-sort has a preview mode and undo**, and no longer touches hidden
  files, `.git`/`node_modules` folders or already-sorted files.
- **Image sub-categories work out of the box.** Downloading the model also
  downloads ONNX Runtime, classification quality improved substantially, and
  camera photos are finally sub-classified.

### Fixed

- Image near-duplicates were often missed: candidate pairs were only compared
  if the first 8 characters of their hash string matched exactly. Replaced with
  pigeonhole LSH banding, which guarantees no qualifying pair is missed.
- Video similarity was normalized by 64 instead of the hash's 1000 bits, so
  re-encoded videos almost never matched.
- CLI: `delete --keep oldest/newest` could keep a file that no longer
  existed and delete the only remaining copy.
- GUI: pausing a scan dropped it and resuming started a second scan from
  scratch, while the first kept running.
- GUI: "Select All" plus "Delete Selected" permanently deleted every copy with
  no confirmation.
- GUI: crashes on file names or text previews containing non-ASCII
  characters (string slicing on byte offsets).
- GUI: the image/PDF/video preview spawned external processes and reloaded the
  syntax-highlighting definitions on every redraw; thumbnails decoded
  full-resolution images inside the render loop.
- GUI: "Open folder" only worked on Linux (`xdg-open`); now works on Windows
  and macOS too.
- GUI: model download wrote directly to the final file, so an interrupted
  download looked installed; cancelling deleted an already-installed model.
- ML: the app could crash when ONNX Runtime was missing (`ort` panics), hang
  forever when it was too old (deadlock inside `ort`), or segfault on exit.
  All three are handled now.
- ML: `ort`'s default features required ONNX Runtime 1.24+, but Windows builds
  bundled 1.21, so ML sorting on Windows hung. Now any runtime ≥ 1.18 works.
- ML: CLIP confidences were raw cosine similarities (≈0.2-0.3), so every image
  "failed" the 0.45 threshold and was escalated to the cloud if configured.
  Now calibrated softmax probabilities.
- ML: images were squashed to 224×224 instead of CLIP's resize + center-crop.
- ML: any photo with camera EXIF stopped at `photo_general`, and any 1080p
  image became a "desktop screenshot", before CLIP was consulted.
- Images with the wrong extension (e.g. a PNG named `.jpg`) failed to decode.
- Auto-sort skipped an entire source folder if any entry in it was unreadable,
  renamed already-sorted files when run twice, and gave clashing destinations
  in dry runs.
- CLI: `-t image,document` (as documented) was rejected; comma-separated and
  plural forms now work.
- The cloud settings in the GUI were never used, and "Anthropic" had no
  implementation.
- "Scanner threads" and "thumbnail cache size" settings did nothing (the first
  now works, the second was removed).
- Release builds for Linux and macOS skipped their bundling step (it looked for
  `.tar.gz`, the archives are `.tar.xz`); the bundled FFmpeg libraries were
  never used anyway (videos are read through the `ffmpeg` command-line tool),
  and are gone, which makes the ~105 MB Windows downloads much smaller.
- The documented `brew install` command could not work (no tap); removed.
- The LICENSE file linked from the README was missing.

### Added

- Cloud classification via **Ollama, OpenAI, Google Gemini and Anthropic
  Claude**, with per-provider model settings, API keys from environment
  variables, images downscaled to 768 px before upload, and a request cap.
- CLIP prototypes built from prompt ensembles
  (`tools/gen_clip_prototypes.py`); 16/16 instead of 11/16 on the test set.
- CLI: `shingan undo-sort`, `shingan model status|download|remove`,
  `shingan doctor`, `delete --permanent/--yes`, `scan --min-size-kb/--threads`,
  `sort --cloud/--skip-unknown`, `--keep shortest-path`, `--version`.
- GUI: recent scans list (reopen or forget past results), keep
  Oldest/Newest/Largest/Shortest-path buttons, confirmation bar, background
  thumbnails with a disk cache, PDF text fallback without poppler, sort
  preview list, stop button and undo, Esc to close previews, folders as
  command-line arguments (`shingan-gui ~/Pictures`), settings validation and
  reset, theme remembered between runs.
- Archives with a unique size are no longer hashed (they can't have a duplicate).
- Best-effort text extraction for legacy `.doc` files; RTF control words are stripped.
- `pdf-extract` panics on malformed PDFs no longer abort a scan.
- Settings are shared by the CLI and the GUI. `settings.json` is written with
  owner-only permissions on Unix.

### Changed

- Similarity for documents and code is now computed on word/token shingles
  rather than characters; thresholds behave a little differently from 0.1.0.
- Image detection uses a real DCT pHash instead of a block hash, and decodes
  each image once instead of three times.
- Scanning and grouping run in parallel, with deterministic output order.
- `delete` defaults to `--keep oldest` (usually the original) and the trash.
- The default "minimum confidence" for image sub-folders is 0.20 and now means
  "below this, file as `other`".
- Minimum supported Rust version is 1.88 (was incorrectly stated as 1.80).

## [0.1.0] - 2026-04-01

Initial release.

[0.2.0]: https://github.com/rc-basilisk/shingan/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/rc-basilisk/shingan/releases/tag/v0.1.0
