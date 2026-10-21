//! End-to-end scans over a temporary directory.

use shingan_core::detector::archive::ArchiveDetector;
use shingan_core::detector::code::CodeDetector;
use shingan_core::detector::document::DocumentDetector;
use shingan_core::detector::image::ImageDetector;
use shingan_core::detector::Detector;
use shingan_core::file_info::{FileCategory, FileInfo};
use shingan_core::scanner::duplicate::{DuplicateScanner, ScanControl, SignatureCache};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

fn scan(
    dir: &Path,
    category: FileCategory,
    detector: Box<dyn Detector>,
    cache: Option<Box<dyn SignatureCache>>,
) -> shingan_core::scanner::duplicate::ScanOutcome {
    let mut detectors: HashMap<FileCategory, Box<dyn Detector>> = HashMap::new();
    detectors.insert(category, detector);
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut scanner = DuplicateScanner::new(
        &[category],
        detectors,
        0.9,
        Arc::new(ScanControl::new()),
        tx,
    )
    .with_size_limits(1, None);
    if let Some(c) = cache {
        scanner = scanner.with_signature_cache(c);
    }
    scanner.scan_paths(&[(dir.to_path_buf(), true)])
}

fn prose(n: usize) -> String {
    (0..n)
        .map(|i| format!("This is sentence {i} of a fairly ordinary report. "))
        .collect()
}

#[test]
fn finds_near_duplicate_documents() {
    let dir = tempfile::tempdir().unwrap();
    let body = prose(200);
    std::fs::write(dir.path().join("report.txt"), &body).unwrap();
    std::fs::write(
        dir.path().join("report (edited).txt"),
        body.replacen("sentence 12 ", "sentence twelve ", 1),
    )
    .unwrap();
    std::fs::write(dir.path().join("other.txt"), "completely different words ".repeat(50))
        .unwrap();

    let (groups, new_sigs) = scan(
        dir.path(),
        FileCategory::Document,
        Box::new(DocumentDetector::new(0.9)),
        None,
    );
    let groups = &groups[&FileCategory::Document];
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0].files.len(), 2);
    assert!(groups[0].similarity > 0.9 && groups[0].similarity < 1.0);
    assert_eq!(new_sigs.len(), 3);
}

#[test]
fn finds_reformatted_code() {
    let dir = tempfile::tempdir().unwrap();
    let code: String = (0..40)
        .map(|i| format!("def handler_{i}(event):\n    return event['value'] * {i}\n\n"))
        .collect();
    std::fs::write(dir.path().join("a.py"), &code).unwrap();
    std::fs::write(
        dir.path().join("b.py"),
        format!("# copied from a.py\n{}", code.replace("    return", "        return")),
    )
    .unwrap();

    let (groups, _) = scan(
        dir.path(),
        FileCategory::Code,
        Box::new(CodeDetector::new(0.9)),
        None,
    );
    assert_eq!(groups[&FileCategory::Code].len(), 1);
}

#[test]
fn finds_resized_images() {
    let dir = tempfile::tempdir().unwrap();
    let img = image::RgbImage::from_fn(300, 200, |x, y| {
        image::Rgb([(x % 256) as u8, (y * 255 / 200) as u8, ((x * y) % 97) as u8])
    });
    img.save(dir.path().join("big.png")).unwrap();
    image::imageops::resize(&img, 150, 100, image::imageops::FilterType::Triangle)
        .save(dir.path().join("small.png"))
        .unwrap();
    let noise = image::RgbImage::from_fn(300, 200, |x, y| {
        let v = ((x * 7919 + y * 104729) % 251) as u8;
        image::Rgb([v, v.wrapping_mul(3), v.wrapping_mul(7)])
    });
    noise.save(dir.path().join("unrelated.png")).unwrap();

    let (groups, _) = scan(
        dir.path(),
        FileCategory::Image,
        Box::new(ImageDetector::new(0.9, 12)),
        None,
    );
    let groups = &groups[&FileCategory::Image];
    assert_eq!(groups.len(), 1, "{groups:?}");
    let names: Vec<_> = groups[0]
        .files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["big.png", "small.png"]);
}

#[test]
fn archives_with_unique_sizes_are_not_hashed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.zip"), vec![1u8; 2000]).unwrap();
    std::fs::write(dir.path().join("b.zip"), vec![1u8; 2000]).unwrap();
    std::fs::write(dir.path().join("c.zip"), vec![2u8; 3000]).unwrap();

    let (groups, new_sigs) = scan(
        dir.path(),
        FileCategory::Archive,
        Box::new(ArchiveDetector::new(1.0)),
        None,
    );
    assert_eq!(groups[&FileCategory::Archive].len(), 1);
    // c.zip has a unique size, so it was never read.
    assert_eq!(new_sigs.len(), 2);
}

/// In-memory cache that records lookups.
struct MemCache(Arc<Mutex<HashMap<String, String>>>);

impl SignatureCache for MemCache {
    fn lookup(&self, _category: FileCategory, files: &[&FileInfo]) -> HashMap<String, String> {
        let map = self.0.lock().unwrap();
        files
            .iter()
            .filter_map(|f| {
                let key = f.path.to_string_lossy().into_owned();
                map.get(&key).map(|s| (key, s.clone()))
            })
            .collect()
    }
}

#[test]
fn cached_signatures_are_reused() {
    let dir = tempfile::tempdir().unwrap();
    let body = prose(100);
    std::fs::write(dir.path().join("one.txt"), &body).unwrap();
    std::fs::write(dir.path().join("two.txt"), &body).unwrap();

    let store = Arc::new(Mutex::new(HashMap::new()));
    let (_, first) = scan(
        dir.path(),
        FileCategory::Document,
        Box::new(DocumentDetector::new(0.9)),
        Some(Box::new(MemCache(store.clone()))),
    );
    assert_eq!(first.len(), 2);
    for s in first {
        store.lock().unwrap().insert(s.path, s.signature);
    }

    let (groups, second) = scan(
        dir.path(),
        FileCategory::Document,
        Box::new(DocumentDetector::new(0.9)),
        Some(Box::new(MemCache(store))),
    );
    assert!(second.is_empty(), "everything should come from cache");
    assert_eq!(groups[&FileCategory::Document].len(), 1);
}
