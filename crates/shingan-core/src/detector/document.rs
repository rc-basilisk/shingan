use crate::detector::{minhash, Detector, PreparedSignatures};
use crate::error::Result;
use crate::file_info::FileCategory;
use std::io::Read;
use std::path::Path;

/// Document duplicate detector using text extraction and MinHash.
///
/// Extracts text from TXT/SRT/VTT/SUB, RTF, DOC, DOCX, ODT, and PDF files,
/// lowercases it, and builds a MinHash sketch over 3-word shingles. Similarity
/// is the Sorensen-Dice coefficient of the shingle sets (see
/// [`crate::detector::minhash`]).
pub struct DocumentDetector {
    threshold: f64,
}

impl DocumentDetector {
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }

    /// Extract human-readable text from a supported document, as-is (not
    /// normalized). Useful for previews.
    pub fn readable_text(path: &Path) -> Option<String> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        match ext.as_str() {
            "txt" | "srt" | "vtt" | "sub" => Self::extract_plain_text(path),
            "rtf" => Self::extract_plain_text(path).map(|t| strip_rtf(&t)),
            "docx" => Self::extract_docx(path),
            // Legacy .doc is an OLE container; some files named .doc are really DOCX.
            "doc" => Self::extract_docx(path).or_else(|| Self::extract_legacy_doc(path)),
            "odt" => Self::extract_odt(path),
            "pdf" => Self::extract_pdf(path),
            _ => None,
        }
    }

    /// Extracted text, whitespace-normalized and lowercased for comparison.
    fn extract_text(&self, path: &Path) -> Option<String> {
        let text = Self::readable_text(path);

        if let Some(ref text) = text {
            let normalized: String = text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            if normalized.len() < 10 {
                return None;
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
        // pdf-extract can panic on malformed files; don't let one bad PDF kill the scan.
        std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes))
            .ok()?
            .ok()
    }

    /// Best-effort text from a Word 97-2003 binary document: collect runs of
    /// printable text stored either as 8-bit characters or UTF-16LE.
    fn extract_legacy_doc(path: &Path) -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        let mut out = String::new();

        // UTF-16LE runs
        let mut run = String::new();
        for pair in bytes.as_chunks::<2>().0 {
            let c = u16::from_le_bytes(*pair);
            match char::from_u32(c as u32) {
                Some(ch) if !ch.is_control() || ch == ' ' => run.push(ch),
                _ => flush_run(&mut run, &mut out),
            }
        }
        flush_run(&mut run, &mut out);

        // 8-bit runs
        for &b in &bytes {
            if b.is_ascii_graphic() || b == b' ' {
                run.push(b as char);
            } else {
                flush_run(&mut run, &mut out);
            }
        }
        flush_run(&mut run, &mut out);

        if out.trim().is_empty() {
            None
        } else {
            Some(out)
        }
    }
}

/// Keep runs of at least 8 characters containing a space (likely prose).
fn flush_run(run: &mut String, out: &mut String) {
    if run.chars().count() >= 8 && run.contains(' ') {
        out.push_str(run);
        out.push(' ');
    }
    run.clear();
}

/// Very small RTF-to-text converter: drops control words, groups and braces.
fn strip_rtf(rtf: &str) -> String {
    let mut out = String::with_capacity(rtf.len() / 2);
    let mut chars = rtf.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                match chars.peek() {
                    Some('\\') | Some('{') | Some('}') => {
                        out.push(chars.next().unwrap_or(' '));
                    }
                    _ => {
                        // Skip control word and an optional numeric parameter.
                        while chars.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                            chars.next();
                        }
                        while chars
                            .peek()
                            .is_some_and(|c| c.is_ascii_digit() || *c == '-')
                        {
                            chars.next();
                        }
                        if chars.peek() == Some(&' ') {
                            chars.next();
                        }
                        out.push(' ');
                    }
                }
            }
            '{' | '}' => {}
            _ => out.push(c),
        }
    }
    out
}

impl Detector for DocumentDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        Ok(self
            .extract_text(path)
            .and_then(|text| minhash::signature_from_tokens(text.split_whitespace(), 3)))
    }

    fn compare_signatures(&self, sig1: &str, sig2: &str) -> f64 {
        minhash::compare(sig1, sig2)
    }

    fn candidate_keys(&self, sig: &str, _threshold: f64) -> Vec<u64> {
        minhash::band_keys(sig)
    }

    fn prepare<'a>(&'a self, sigs: &[&'a str]) -> Box<dyn PreparedSignatures + 'a> {
        Box::new(minhash::PreparedMinHash::new(sigs))
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

    #[test]
    fn near_duplicate_documents_match() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (0..300)
            .map(|i| format!("sentence number {i} goes here. "))
            .collect();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, &body).unwrap();
        std::fs::write(&b, body.replacen("number 7", "NUMBER seven", 1)).unwrap();
        let det = DocumentDetector::new(0.9);
        let sa = det.compute_signature(&a).unwrap().unwrap();
        let sb = det.compute_signature(&b).unwrap().unwrap();
        assert!(det.compare_signatures(&sa, &sb) > 0.95);
    }

    #[test]
    fn rtf_control_words_are_stripped() {
        let text = strip_rtf(r"{\rtf1\ansi{\fonttbl\f0 Arial;}\f0\fs24 Hello \b world\b0 !}");
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
        assert!(!text.contains("fs24"));
    }
}
