# How shingan works

This page explains the algorithms behind duplicate detection and image
classification. You don't need any of this to use shingan; see the
[README](../README.md) for that.

## Duplicate detection

A scan has three phases.

1. **Discovery.** The folders are walked (`walkdir`) and files are bucketed by
   extension into images, documents, videos, archives and code. Version-control
   and tool folders (`.git`, `.svn`, `node_modules`, `build`, `dist`,
   virtualenvs, Python caches) are skipped, symlinks are not followed, and files outside the size limits
   (default: smaller than 1 KiB) are ignored.
2. **Fingerprinting.** Every file gets a *signature*, computed in parallel
   with `rayon`. Signatures are cached in SQLite keyed by
   `(path, size, modification time, file type)`, so unchanged files are not
   re-read on the next scan.
3. **Grouping.** Signatures are compared and similar files are clustered.

### Signatures per file type

| Type | Signature | Similarity |
|---|---|---|
| Image | Three 12×12-bit perceptual hashes: average hash, DCT hash (pHash) and gradient hash (dHash), from one decode | `1 − hamming/144` for each hash, then the **minimum** of the three, so all three must agree |
| Video | `vid_dup_finder_lib`'s 1000-bit 3-D DCT hash over a sampled window (skip the first 3 s, sample 20 s by default), via the `ffmpeg` CLI | `1 − hamming/1000` |
| Document | Text extraction (PDF, DOCX, ODT, legacy DOC best-effort, RTF stripped, plain text), lowercased, then **MinHash** (128 hashes) over 3-word shingles | Estimated Jaccard `J` of the shingle sets, reported as Sørensen-Dice `2J/(1+J)` |
| Code | Comment-only lines and whitespace removed, tokenized into identifiers, numbers and punctuation, then MinHash over 4-token shingles | Same as documents |
| Archive | SHA-256 of the bytes (only for files whose size occurs more than once) | Exact match only |

Before v0.2.0, document and code signatures were SHA-256 hashes of the
normalized text, so "fuzzy" matching of text silently degraded to exact
matching. MinHash fixes this: it can be compared from cached signatures alone,
without re-reading files.

### Finding candidate pairs (LSH)

Comparing every pair is O(n²), so each detector provides
locality-sensitive **bucket keys** (`Detector::candidate_keys`), and only files
sharing a key are compared:

- **Images:** pigeonhole banding on the dHash. At threshold `t` two hashes may
  differ in at most `d = ⌊(1−t)·144⌋` bits. Splitting the hash into `d+1`
  contiguous bands guarantees that a qualifying pair shares at least one
  identical band, so no true match is ever missed.
- **Documents/code:** classic MinHash banding, 32 bands of 4 rows. A pair with
  Jaccard 0.8 becomes a candidate with probability above 99.9%; unrelated
  texts almost never do.
- **Videos:** all pairs are compared (cheap once the hashes are parsed).
- **Archives:** the hash itself is the key.

Candidate pairs are evaluated in parallel. A pair appearing in several buckets
is evaluated only in the first bucket the two files share, which avoids a
memory-hungry "seen pairs" set.

### Clustering

Similar pairs are sorted from most to least similar and merged with a
union-find structure. A merge is only accepted if **every** file of one cluster
is similar to **every** file of the other. This prevents chaining (A≈B and B≈C,
but A≉C). A group's reported similarity is its weakest pair. Output order is
deterministic.

## Image classification (Auto-sort sub-folders)

Images can be classified into 24 categories (see the README). There are four
tiers:

| Tier | Method | Notes |
|---|---|---|
| 0 | Heuristics on metadata | Icon-sized images, desktop/mobile screen resolutions, camera EXIF (make, model, GPS, focal length, scanner software) |
| 1 | Pixel statistics | Brightness and edge energy, which flag diagrams and scans |
| 2 | Local CLIP ViT-B/32 (ONNX Runtime) | Zero-shot: image embedding versus one text prototype per category |
| 3 | Cloud vision model (optional) | Ollama, OpenAI, Gemini or Anthropic, for images the local tiers are unsure of |

**With the CLIP model installed**, every image except tiny icons goes through
CLIP. The tier-0 guess is used as a *prior*: it multiplies the matching
category's probability (and camera EXIF boosts all `photo_*` categories), so
a camera photo becomes `photo_food` rather than a generic `photo_general`.
CLIP cosine similarities are scaled by CLIP's logit scale (100) and passed
through a softmax, giving calibrated probabilities. If the best probability
is low and a cloud provider is configured, the image is escalated, within the
per-sort request budget.

**Without CLIP**, tiers 0 and 1 run in order, then the cloud (if configured),
then relaxed versions of 0 and 1.

Results below *Minimum confidence* (Settings, default 0.20) are filed as `other`.

### The CLIP prototypes

`crates/shingan-ml/data/clip_prototypes.bin` holds 24 × 512 float32 text
embeddings, compiled into the binary. Each one averages the CLIP text
embeddings of several short prompts ("a photo of a dog", "a wildlife photo",
...), as in the CLIP paper's prompt ensembling. They are generated by
[`tools/gen_clip_prototypes.py`](../tools/gen_clip_prototypes.py) from the
[Xenova/clip-vit-base-patch32](https://huggingface.co/Xenova/clip-vit-base-patch32)
text encoder. On a small mixed test set, ensembling raised accuracy from 11/16
(the earlier single long prompts) to 16/16.

To experiment, put your own `clip_prototypes.bin` next to `clip_image.onnx` in
the models folder. It takes precedence over the built-in copy.

Image preprocessing follows CLIP: resize the shortest side to 224 px
(bicubic), center-crop to 224×224, and normalize with the CLIP mean/std.

### ONNX Runtime

The `ort` crate loads ONNX Runtime dynamically, so the binaries start even
without it. shingan looks for the library, in order, at `ORT_DYLIB_PATH`,
next to the executable (and in `lib/` beside it), in the models folder, and in
the system library path. Libraries older than 1.18 are rejected before they
reach `ort`, because `ort` deadlocks when initialized with a runtime that is
too old. Downloading the model also downloads ONNX Runtime 1.22 into the
models folder when no usable runtime is found.

## Storage

- **SQLite** (`shingan.db`, WAL mode, 5 s busy timeout so the CLI and the app
  can run at the same time). Tables: `scan_sessions`, `duplicate_groups`,
  `file_entries`, `signature_cache`, plus sorting tables reserved for future
  use. The schema version is kept in `PRAGMA user_version`; upgrading
  discards cached signatures whose format changed.
- **Sort journals** (`.shingan-sort-<timestamp>.json` in the destination)
  record every move so a sort can be undone.
- **Thumbnails and previews** are cached in the OS cache folder, keyed by
  path, size and modification time.
