use crate::detector::{minhash, Detector, PreparedSignatures};
use crate::error::Result;
use crate::file_info::FileCategory;
use std::path::Path;

/// Code duplicate detector using normalization and MinHash.
///
/// Normalizes code by stripping comment-only lines and whitespace, tokenizes it
/// into identifiers, numbers and punctuation, and builds a MinHash sketch over
/// 4-token shingles. Formatting changes and comment edits therefore don't
/// affect the score; renamed identifiers lower it only locally.
pub struct CodeDetector {
    threshold: f64,
}

impl CodeDetector {
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }

    /// Read and normalize code: drop blank and comment-only lines.
    fn normalize_code(path: &Path) -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        let content = String::from_utf8_lossy(&bytes);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        // Only languages where `#` starts a comment (not C preprocessor lines).
        let hash_comments = matches!(ext.as_str(), "py" | "exs" | "rb" | "sh");

        let mut in_block = false;
        let mut out = String::with_capacity(content.len());
        for line in content.lines() {
            let line = line.trim();
            if in_block {
                if line.contains("*/") {
                    in_block = false;
                }
                continue;
            }
            if line.is_empty()
                || line.starts_with("//")
                || (hash_comments && line.starts_with('#'))
                || line.starts_with("<!--")
            {
                continue;
            }
            if line.starts_with("/*") {
                in_block = !line.contains("*/");
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }

        if out.trim().is_empty() {
            None
        } else {
            Some(out)
        }
    }
}

/// Split code into identifier/number runs and single punctuation characters.
fn tokenize(code: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in code.char_indices() {
        if c.is_alphanumeric() || c == '_' {
            if start.is_none() {
                start = Some(i);
            }
        } else {
            if let Some(s) = start.take() {
                tokens.push(&code[s..i]);
            }
            if !c.is_whitespace() {
                tokens.push(&code[i..i + c.len_utf8()]);
            }
        }
    }
    if let Some(s) = start {
        tokens.push(&code[s..]);
    }
    tokens
}

impl Detector for CodeDetector {
    fn compute_signature(&self, path: &Path) -> Result<Option<String>> {
        Ok(Self::normalize_code(path)
            .and_then(|code| minhash::signature_from_tokens(tokenize(&code), 4)))
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
        FileCategory::Code
    }

    fn threshold(&self) -> f64 {
        self.threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_missing_file() {
        let result = CodeDetector::normalize_code(Path::new("/nonexistent/path/file.py"));
        assert!(result.is_none());
    }

    #[test]
    fn tokenize_splits_identifiers_and_punctuation() {
        assert_eq!(
            tokenize("fn main() { x_1 += 2; }"),
            vec!["fn", "main", "(", ")", "{", "x_1", "+", "=", "2", ";", "}"]
        );
    }

    #[test]
    fn reformatted_code_matches() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.rs");
        let b = dir.path().join("b.rs");
        let body: String = (0..60)
            .map(|i| format!("fn f{i}(x: u32) -> u32 {{ x + {i} }}\n"))
            .collect();
        std::fs::write(&a, &body).unwrap();
        // Same code, different indentation and an extra comment.
        let reformatted = format!(
            "// header comment\n{}",
            body.replace("{ x", "{\n    x").replace(" }\n", "\n}\n")
        );
        std::fs::write(&b, reformatted).unwrap();
        let det = CodeDetector::new(0.9);
        let sa = det.compute_signature(&a).unwrap().unwrap();
        let sb = det.compute_signature(&b).unwrap().unwrap();
        assert!(det.compare_signatures(&sa, &sb) > 0.99);
    }

    #[test]
    fn c_preprocessor_lines_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.c");
        std::fs::write(&p, "#include <stdio.h>\nint main() { return 0; }\n").unwrap();
        let code = CodeDetector::normalize_code(&p).unwrap();
        assert!(code.contains("#include"));
    }
}
