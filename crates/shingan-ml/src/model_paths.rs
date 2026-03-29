//! Filesystem locations and validation for ONNX model files.
//!
//! The Tier 2 CLIP classifier requires `clip_image.onnx` (the CLIP ViT-B/32
//! image encoder in ONNX format) to be present on disk.  The category prototype
//! vectors (`clip_prototypes.bin`) are embedded in the binary at compile time;
//! an on-disk copy in the same directory is used if present (for
//! experimentation) but is not required.
//!
//! By default model files are stored under `$XDG_DATA_HOME/shingan/models/`
//! (typically `~/.local/share/shingan/models/` on Linux). The functions in this
//! module provide the canonical paths and a [`validate_prototype_file`] helper
//! that checks file size matches the expected `24 × embed_dim × 4` bytes.
//!
//! [`EXPECTED_PROTOTYPE_ROWS`] is derived at compile time from
//! [`ImageSubCategory::ALL`](crate::taxonomy::ImageSubCategory::ALL) so it
//! stays in sync with the taxonomy automatically.

use std::path::{Path, PathBuf};

/// `$XDG_DATA_HOME/shingan/models` or platform equivalent.
pub fn default_models_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("shingan")
        .join("models")
}

/// Full path to the CLIP image encoder ONNX model file.
pub fn clip_image_onnx_path() -> PathBuf {
    default_models_dir().join("clip_image.onnx")
}

/// Full path to the precomputed CLIP text prototype vectors.
pub fn clip_prototypes_path() -> PathBuf {
    default_models_dir().join("clip_prototypes.bin")
}

/// Expected float32 bytes: `N * EMBED_DIM * 4` where `N == ImageSubCategory::ALL.len()`.
pub const EXPECTED_PROTOTYPE_ROWS: usize = crate::taxonomy::ImageSubCategory::ALL.len();

/// Check that a prototype file on disk has the expected byte size for the given embedding dimension.
pub fn validate_prototype_file(path: &Path, embed_dim: usize) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let expected = EXPECTED_PROTOTYPE_ROWS * embed_dim * 4;
    meta.len() == expected as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn default_models_dir_ends_with_expected_segments() {
        let dir = default_models_dir();
        assert!(dir.ends_with("shingan/models"), "got: {}", dir.display());
    }

    #[test]
    fn default_models_dir_is_absolute_or_relative_dot() {
        let dir = default_models_dir();
        assert!(
            dir.is_absolute() || dir.starts_with("."),
            "expected absolute or dot-relative, got: {}",
            dir.display()
        );
    }

    #[test]
    fn clip_paths_are_under_models_dir() {
        let base = default_models_dir();
        let onnx = clip_image_onnx_path();
        let proto = clip_prototypes_path();
        assert_eq!(onnx.parent().unwrap(), base);
        assert_eq!(proto.parent().unwrap(), base);
        assert!(onnx.to_string_lossy().ends_with("clip_image.onnx"));
        assert!(proto.to_string_lossy().ends_with("clip_prototypes.bin"));
    }

    #[test]
    fn expected_prototype_rows_matches_taxonomy() {
        assert_eq!(EXPECTED_PROTOTYPE_ROWS, 24);
        assert_eq!(
            EXPECTED_PROTOTYPE_ROWS,
            crate::taxonomy::ImageSubCategory::ALL.len()
        );
    }

    #[test]
    fn validate_prototype_file_nonexistent() {
        assert!(!validate_prototype_file(
            Path::new("/tmp/shingan_test_nonexistent_file_xyz.bin"),
            512
        ));
    }

    #[test]
    fn validate_prototype_file_correct_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proto.bin");
        let embed_dim = 512;
        let expected_bytes = EXPECTED_PROTOTYPE_ROWS * embed_dim * 4;
        let data = vec![0u8; expected_bytes];
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&data).unwrap();
        assert!(validate_prototype_file(&path, embed_dim));
    }

    #[test]
    fn validate_prototype_file_wrong_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proto_bad.bin");
        let data = vec![0u8; 100];
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&data).unwrap();
        assert!(!validate_prototype_file(&path, 512));
    }

    #[test]
    fn validate_prototype_file_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.bin");
        std::fs::File::create(&path).unwrap();
        assert!(!validate_prototype_file(&path, 512));
    }

    #[test]
    fn validate_prototype_file_different_embed_dims() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proto.bin");
        let embed_dim = 256;
        let expected_bytes = EXPECTED_PROTOTYPE_ROWS * embed_dim * 4;
        let data = vec![0u8; expected_bytes];
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&data).unwrap();

        assert!(validate_prototype_file(&path, 256));
        assert!(!validate_prototype_file(&path, 512));
    }
}
