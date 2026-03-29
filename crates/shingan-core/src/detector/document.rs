use crate::cache::BoundedCache;
use crate::detector::Detector;
use crate::error::Result;
use crate::file_info::FileCategory;
use parking_lot::Mutex;
use std::io::Read;
use std::path::Path;

/// Document duplicate detector using text extraction and fuzzy matching.
///
/// Extracts text from TXT, DOCX, ODT, and PDF files, then uses
/// `token_sort_ratio` for fuzzy comparison.
pub struct DocumentDetector {
    threshold: f64,
    cache: Mutex<BoundedCache<String, String>>,
}

impl DocumentDetector {
    pub fn new(threshold: f64) -> Self {
        Self {
            threshold,
            cache: Mutex::new(BoundedCache::new(1000)),
        }
    }

    /// Extract text content from a document file.
    fn extract_text(&self, path: &Path) -> Option<String> {
        let key = path.to_string_lossy().to_string();

        {
            let mut cache = self.cache.lock();
            if let Some(text) = cache.get(&key) {
                return Some(text.clone());
            }
        }

        let ext = path.extension()?.to_str()?.to_lowercase();
        let text = match ext.as_str() {
            "txt" | "srt" | "vtt" | "sub" | "rtf" => Self::extract_plain_text(path),
            "docx" | "doc" => Self::extract_docx(path),
            "odt" => Self::extract_odt(path),
            "pdf" => Self::extract_pdf(path),
            _ => None,
        };

        if let Some(ref text) = text {
            let normalized: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if normalized.len() < 10 {
                return None;
            }

            {
                let mut cache = self.cache.lock();
                cache.put(key, normalized.clone());
            }
            return Some(normalized);
        }

        text
    }

    fn extract_plain_text(path: &Path) -> Option<String> {
        // Try UTF-8 first, fall back to lossy
        if let Ok(content) = std::fs::read_to_string(path) {
            return Some(content);
        }
        if let Ok(bytes) = std::fs::read(path) {
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
        None
    }

    /// Extract text from DOCX (ZIP containing word/document.xml).
    fn extract_docx(path: &Path) -> Option<String> {
        let file = std::fs::File::open(path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;

        let mut doc_xml = String::new();
        {
            let mut entry = archive.by_name("word/document.xml").ok()?;
            entry.read_to_string(&mut doc_xml).ok()?;
        }

        // Parse XML and extract text from <w:t> elements
        let mut reader = quick_xml::Reader::from_str(&doc_xml);
        let mut text_parts = Vec::new();
        let mut in_wt = false;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(quick_xml::events::Event::Start(ref e))
                | Ok(quick_xml::events::Event::Empty(ref e)) => {
                    if e.local_name().as_ref() == b"t" {
                        in_wt = true;
                    }
                }
                Ok(quick_xml::events::Event::Text(ref e)) => {
                    if in_wt {
                        if let Ok(text) = e.unescape() {
                            text_parts.push(text.into_owned());
                        }
                    }
                }
                Ok(quick_xml::events::Event::End(ref e)) => {
                    if e.local_name().as_ref() == b"t" {
                        in_wt = false;
                    }
                    // Add space after paragraph elements
                    if e.local_name().as_ref() == b"p" {
                        text_parts.push("\n".to_string());
                    }
                }
                Ok(quick_xml::events::Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        let result = text_parts.join("");
        if result.trim().is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Extract text from ODF/ODT (ZIP containing content.xml).
    fn extract_odt(path: &Path) -> Option<String> {
        let file = std::fs::File::open(path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;

        let mut content_xml = String::new();
        {
            let mut entry = archive.by_name("content.xml").ok()?;
            entry.read_to_string(&mut content_xml).ok()?;
        }

        // Parse XML and extract text from <text:p> elements
        let mut reader = quick_xml::Reader::from_str(&content_xml);
        let mut text_parts = Vec::new();
        let mut depth_in_p = 0u32;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(quick_xml::events::Event::Start(ref e)) => {
                    if e.local_name().as_ref() == b"p" {
                        depth_in_p += 1;
                    }
                }
                Ok(quick_xml::events::Event::Text(ref e)) => {
                    if depth_in_p > 0 {
                        if let Ok(text) = e.unescape() {
                            text_parts.push(text.into_owned());
                        }
                    }
                }
                Ok(quick_xml::events::Event::End(ref e)) => {
                    if e.local_name().as_ref() == b"p" && depth_in_p > 0 {
                        depth_in_p -= 1;
                        text_parts.push("\n".to_string());
                    }
                }
                Ok(quick_xml::events::Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        let result = text_parts.join("");
        if result.trim().is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Extract text from PDF using pdf-extract.
    fn extract_pdf(path: &Path) -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        pdf_extract::extract_text_from_mem(&bytes).ok()
    }
}

impl Detector for DocumentDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        match self.extract_text(path) {
            Some(text) => {
                use sha2::{Digest, Sha256};
                let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
                Ok(Some(hash))
            }
            None => Ok(None),
        }
    }

    fn compare_files(&self, file1: &Path, file2: &Path) -> Result<f64> {
        let text1 = self.extract_text(file1);
        let text2 = self.extract_text(file2);

        match (text1, text2) {
            (Some(t1), Some(t2)) => Ok(strsim::sorensen_dice(&t1, &t2)),
            _ => Ok(0.0),
        }
    }

    fn category(&self) -> FileCategory {
        FileCategory::Document
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_signature_missing_file() {
        let det = DocumentDetector::new(0.9);
        let result = det.compute_signature(Path::new("/nonexistent/path/file.txt"));
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
