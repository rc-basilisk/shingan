//! File metadata enrichment for the scanning and classification pipelines.
//!
//! After a [`crate::file_info::FileInfo`] is constructed from filesystem
//! metadata, this module can enrich it with content-derived signals that are
//! too expensive to compute during initial directory walking but are needed by
//! both the duplicate detection engine and the ML classification pipeline
//! (`shingan_ml`):
//!
//! - **MIME type** ([`sniff_mime_type`]) — magic-byte sniffing via the `infer`
//!   crate with an extension-based fallback through `mime_guess`.
//! - **Image dimensions** ([`image_dimensions`]) — decoded from image headers
//!   without a full pixel decode.
//! - **EXIF camera signals** ([`image_has_meaningful_exif`]) — checks for
//!   camera make/model, GPS coordinates, focal length, or scanner software
//!   tags. This is a key input to Tier 0 heuristic classification.
//!
//! The convenience function [`enrich_image_file_info`] populates all three
//! fields on a `FileInfo` in one call and is a no-op for non-image categories.

use crate::file_info::FileCategory;
use std::io::Read;
use std::path::Path;

/// Read up to `max` bytes from the start of the file for magic-byte sniffing.
pub fn read_file_prefix(path: &Path, max: usize) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; max];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// Sniff MIME type from file contents (magic bytes). Falls back to extension via `mime_guess` if inconclusive.
pub fn sniff_mime_type(path: &Path, extension: &str) -> Option<String> {
    if let Ok(prefix) = read_file_prefix(path, 8192) {
        if let Some(kind) = infer::get(&prefix) {
            return Some(kind.mime_type().to_string());
        }
    }
    mime_guess::from_ext(extension)
        .first_raw()
        .map(|s| s.to_string())
}

/// Decode image width/height without full decode where possible.
pub fn image_dimensions(path: &Path) -> Option<(u32, u32)> {
    let reader = image::ImageReader::open(path).ok()?;
    reader.into_dimensions().ok()
}

/// True if EXIF indicates a camera capture or scanner (not empty / software-only).
pub fn image_has_meaningful_exif(path: &Path) -> bool {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut bufreader = std::io::BufReader::new(file);
    let exifreader = exif::Reader::new();
    let exif = match exifreader.read_from_container(&mut bufreader) {
        Ok(e) => e,
        Err(_) => return false,
    };

    use exif::Tag;

    // Camera make/model strongly implies a photograph.
    if exif.get_field(Tag::Make, exif::In::PRIMARY).is_some()
        || exif.get_field(Tag::Model, exif::In::PRIMARY).is_some()
    {
        return true;
    }

    // GPS or lens info
    if exif.get_field(Tag::GPSLatitude, exif::In::PRIMARY).is_some()
        || exif.get_field(Tag::FocalLength, exif::In::PRIMARY).is_some()
    {
        return true;
    }

    // Scanner software (document scans)
    if let Some(f) = exif.get_field(Tag::Software, exif::In::PRIMARY) {
        let s = f.display_value().to_string().to_lowercase();
        if s.contains("scan") || s.contains("scanner") || s.contains("adobe scan") {
            return true;
        }
    }

    false
}

/// Populate MIME, dimensions, and EXIF flag for a [`crate::file_info::FileInfo`] when it is an image.
pub fn enrich_image_file_info(
    path: &Path,
    extension: &str,
    category: FileCategory,
    mime_type: &mut Option<String>,
    dimensions: &mut Option<(u32, u32)>,
    has_exif: &mut bool,
) {
    if category != FileCategory::Image {
        return;
    }
    *mime_type = sniff_mime_type(path, extension);
    *dimensions = image_dimensions(path);
    *has_exif = image_has_meaningful_exif(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn sniff_png_mime() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.png");
        // Minimal 1x1 PNG
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(png).unwrap();

        let dim = image_dimensions(&p);
        assert_eq!(dim, Some((1, 1)));
        let mime = sniff_mime_type(&p, "png");
        assert!(mime.is_some());
    }
}
