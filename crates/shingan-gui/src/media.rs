//! Thumbnail and preview generation. Everything here is blocking and is run
//! from background tasks, never from `view()`.

use shingan_core::file_info::FileCategory;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Longest side of generated thumbnails, in pixels.
pub const THUMB_PX: u32 = 280;
/// Text previews are cut at this many bytes.
const MAX_PREVIEW_TEXT: usize = 100_000;

fn cache_dir(kind: &str) -> PathBuf {
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("shingan")
        .join(kind);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Cache key derived from path, size and modification time, so edited files
/// get fresh thumbnails.
fn cache_key(path: &Path) -> String {
    let meta = std::fs::metadata(path).ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let key = format!("{}|{size}|{mtime}", path.to_string_lossy());
    format!(
        "{:016x}",
        shingan_core::detector::stable_hash64(key.as_bytes())
    )
}

fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

/// Create (or reuse) a small JPEG thumbnail for an image or video.
pub fn thumbnail(path: &Path, category: FileCategory) -> Option<PathBuf> {
    let out = cache_dir("thumbnails").join(format!("{}.jpg", cache_key(path)));
    if out.is_file() {
        return Some(out);
    }
    match category {
        FileCategory::Image => {
            let img = shingan_ml::open_image(path).ok()?;
            let thumb = img.thumbnail(THUMB_PX, THUMB_PX).to_rgb8();
            thumb.save(&out).ok()?;
            Some(out)
        }
        FileCategory::Video => {
            // Try a few seconds in (skips black intro frames), then the first frame.
            for seek in ["3", "0"] {
                let ok = quiet(
                    Command::new("ffmpeg")
                        .args(["-y", "-ss", seek, "-i"])
                        .arg(path)
                        .args(["-frames:v", "1", "-vf"])
                        .arg(format!("scale={THUMB_PX}:-2"))
                        .arg(&out),
                )
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
                if ok && out.is_file() {
                    return Some(out);
                }
            }
            None
        }
        _ => None,
    }
}

/// One highlighted run of text: content and optional RGBA color.
pub type Span = (String, Option<[u8; 4]>);

/// Everything the preview pane needs, prepared in the background.
#[derive(Debug, Clone, Default)]
pub struct PreviewData {
    pub path: String,
    pub page: usize,
    /// Rendered picture (PDF page or video frame) or the image itself.
    pub image: Option<PathBuf>,
    pub dimensions: Option<(u32, u32)>,
    /// Text content (documents and code), possibly syntax-highlighted.
    pub spans: Option<Vec<Span>>,
    pub truncated: bool,
    pub error: Option<String>,
}

/// Prepare a preview for `path`.
pub fn load_preview(path: &str, category: FileCategory, page: usize, dark: bool) -> PreviewData {
    let p = Path::new(path);
    let mut data = PreviewData {
        path: path.to_string(),
        page,
        ..Default::default()
    };
    let is_pdf = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));

    match category {
        FileCategory::Image => {
            data.image = Some(p.to_path_buf());
            data.dimensions = image::image_dimensions(p).ok().or_else(|| {
                shingan_ml::open_image(p)
                    .ok()
                    .map(|i| (i.width(), i.height()))
            });
        }
        FileCategory::Video => match render_video_frame(p) {
            Some(frame) => {
                data.dimensions = image::image_dimensions(&frame).ok();
                data.image = Some(frame);
            }
            None => {
                data.error =
                    Some("Could not extract a frame. Is ffmpeg installed and on your PATH?".into())
            }
        },
        FileCategory::Document if is_pdf => match render_pdf_page(p, page) {
            Some(img) => {
                data.dimensions = image::image_dimensions(&img).ok();
                data.image = Some(img);
            }
            None => {
                // Fall back to extracted text when poppler isn't available.
                if let Some(t) =
                    shingan_core::detector::document::DocumentDetector::readable_text(p)
                {
                    data.truncated = t.len() > MAX_PREVIEW_TEXT;
                    data.spans = Some(vec![(
                        crate::platform::truncate_bytes(&t, MAX_PREVIEW_TEXT).to_string(),
                        None,
                    )]);
                }
                data.error = Some(
                    "Page images need pdftoppm (poppler-utils); showing extracted text instead."
                        .into(),
                );
            }
        },
        FileCategory::Document => {
            match shingan_core::detector::document::DocumentDetector::readable_text(p) {
                Some(t) => {
                    data.truncated = t.len() > MAX_PREVIEW_TEXT;
                    data.spans = Some(vec![(
                        crate::platform::truncate_bytes(&t, MAX_PREVIEW_TEXT).to_string(),
                        None,
                    )]);
                }
                None => data.error = Some("No readable text in this document.".into()),
            }
        }
        FileCategory::Code => match std::fs::read(p) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                data.truncated = text.len() > MAX_PREVIEW_TEXT;
                let text = crate::platform::truncate_bytes(&text, MAX_PREVIEW_TEXT);
                data.spans = Some(highlight(text, p, dark));
            }
            Err(e) => data.error = Some(format!("Cannot read file: {e}")),
        },
        FileCategory::Archive => {
            data.error =
                Some("Archives can't be previewed; open them with the system instead.".into())
        }
    }
    data
}

fn render_video_frame(path: &Path) -> Option<PathBuf> {
    let out = cache_dir("previews").join(format!("vid_{}.jpg", cache_key(path)));
    if out.is_file() {
        return Some(out);
    }
    for seek in ["5", "0"] {
        let ok = quiet(
            Command::new("ffmpeg")
                .args(["-y", "-ss", seek, "-i"])
                .arg(path)
                .args(["-frames:v", "1", "-q:v", "2"])
                .arg(&out),
        )
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
        if ok && out.is_file() {
            return Some(out);
        }
    }
    None
}

fn render_pdf_page(path: &Path, page: usize) -> Option<PathBuf> {
    let prefix = cache_dir("previews").join(format!("pdf_{}_p{}", cache_key(path), page + 1));
    let out = prefix.with_extension("png");
    if out.is_file() {
        return Some(out);
    }
    let n = (page + 1).to_string();
    let ok = quiet(
        Command::new("pdftoppm")
            .args(["-png", "-r", "110", "-singlefile", "-f", &n, "-l", &n])
            .arg(path)
            .arg(&prefix),
    )
    .status()
    .map(|s| s.success())
    .unwrap_or(false);
    (ok && out.is_file()).then_some(out)
}

fn syntax_set() -> &'static syntect::parsing::SyntaxSet {
    static SET: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
    SET.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static syntect::highlighting::ThemeSet {
    static SET: OnceLock<syntect::highlighting::ThemeSet> = OnceLock::new();
    SET.get_or_init(syntect::highlighting::ThemeSet::load_defaults)
}

/// Syntax-highlight code into colored spans.
fn highlight(code: &str, path: &Path, dark: bool) -> Vec<Span> {
    let ps = syntax_set();
    let theme_name = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let Some(theme) = theme_set().themes.get(theme_name) else {
        return vec![(code.to_string(), None)];
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("txt");
    let syntax = ps
        .find_syntax_by_extension(ext)
        .unwrap_or_else(|| ps.find_syntax_plain_text());
    let mut h = syntect::easy::HighlightLines::new(syntax, theme);
    let mut spans: Vec<Span> = Vec::new();
    for line in syntect::util::LinesWithEndings::from(code) {
        match h.highlight_line(line, ps) {
            Ok(ranges) => {
                for (style, piece) in ranges {
                    let c = style.foreground;
                    // Merge consecutive runs of the same color to keep the widget light.
                    if let Some((last, Some(lc))) = spans.last_mut() {
                        if *lc == [c.r, c.g, c.b, c.a] {
                            last.push_str(piece);
                            continue;
                        }
                    }
                    spans.push((piece.to_string(), Some([c.r, c.g, c.b, c.a])));
                }
            }
            Err(_) => spans.push((line.to_string(), None)),
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_preview_highlights_and_truncates_safely() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.rs");
        std::fs::write(&p, "fn main() { println!(\"心眼\"); }\n".repeat(5000)).unwrap();
        let data = load_preview(&p.to_string_lossy(), FileCategory::Code, 0, true);
        assert!(data.truncated);
        let spans = data.spans.unwrap();
        assert!(spans.len() > 1, "expected several colored spans");
        assert!(spans.iter().map(|s| s.0.len()).sum::<usize>() <= MAX_PREVIEW_TEXT);
    }

    #[test]
    fn image_thumbnail_is_small() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.png");
        image::RgbImage::from_pixel(1200, 600, image::Rgb([1, 2, 3]))
            .save(&p)
            .unwrap();
        let t = thumbnail(&p, FileCategory::Image).unwrap();
        let (w, h) = image::image_dimensions(&t).unwrap();
        assert_eq!((w, h), (THUMB_PX, THUMB_PX / 2));
    }
}
