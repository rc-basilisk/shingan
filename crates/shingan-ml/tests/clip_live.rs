//! Live CLIP check against a real model. Ignored by default; run with
//!
//! ```sh
//! SHINGAN_MODEL_DIR=~/.local/share/shingan/models SHINGAN_SAMPLES=./samples \
//!   cargo test -p shingan-ml --test clip_live -- --ignored --nocapture
//! ```
//!
//! Every `<label>*.jpg|png` in the samples dir is classified and the top-3 printed.

#![cfg(feature = "onnx")]

use shingan_ml::onnx::ClipOnnxClassifier;
use shingan_ml::ImageSubCategory;
use std::path::PathBuf;

#[test]
#[ignore]
fn classify_samples() {
    let model_dir = PathBuf::from(std::env::var("SHINGAN_MODEL_DIR").expect("SHINGAN_MODEL_DIR"));
    let samples = PathBuf::from(std::env::var("SHINGAN_SAMPLES").expect("SHINGAN_SAMPLES"));
    let mut clf = ClipOnnxClassifier::try_load(Some(&model_dir))
        .expect("load")
        .expect("model present");
    let mut files: Vec<_> = std::fs::read_dir(&samples)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    for f in files {
        let probs = match clf.probabilities(&f) {
            Ok(p) => p,
            Err(e) => {
                println!("{:<16} ERROR {e}", f.file_name().unwrap().to_string_lossy());
                continue;
            }
        };
        let mut ranked: Vec<(usize, f32)> = probs.iter().copied().enumerate().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let top: Vec<String> = ranked[..3]
            .iter()
            .map(|(i, p)| format!("{} {:.2}", ImageSubCategory::ALL[*i].label(), p))
            .collect();
        println!(
            "{:<16} {}",
            f.file_name().unwrap().to_string_lossy(),
            top.join(" | ")
        );
    }
}

/// With the model present but no ONNX Runtime anywhere, loading must fail
/// cleanly (no panic, no hang). Run in its own process:
/// `ORT_DYLIB_PATH=/nonexistent SHINGAN_MODEL_DIR=... cargo test --test clip_live missing_runtime -- --ignored`
#[test]
#[ignore]
fn missing_runtime_is_an_error() {
    let model_dir = PathBuf::from(std::env::var("SHINGAN_MODEL_DIR").expect("SHINGAN_MODEL_DIR"));
    let err = ClipOnnxClassifier::try_load(Some(&model_dir))
        .err()
        .expect("should fail without a runtime");
    println!("{err}");
    assert!(err.contains("ONNX Runtime library not found"));
}
