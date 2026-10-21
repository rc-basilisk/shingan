//! Tier 3: optional cloud-based image categorization via remote vision APIs.
//!
//! This module defines the [`CloudCategorizer`] trait — a blocking interface for
//! sending an image to a vision-capable model and receiving back an
//! [`ImageSubCategory`] label. With the `cloud` feature enabled,
//! [`HttpVisionCategorizer`] implements it for four providers:
//!
//! - **Ollama** (`/api/generate`, runs locally, no API key)
//! - **OpenAI** (Chat Completions)
//! - **Google Gemini** (`generateContent`)
//! - **Anthropic Claude** (Messages API)
//!
//! Images are downscaled to at most [`MAX_UPLOAD_SIDE`] pixels and re-encoded
//! as JPEG before upload, which keeps requests small, fast and cheap and avoids
//! sending full-resolution originals off the machine.
//!
//! Cloud categorization is only invoked by the pipeline when local tiers
//! produce confidence below
//! [`PipelineConfig::cloud_escalation_threshold`](crate::pipeline::PipelineConfig::cloud_escalation_threshold),
//! or when the local CLIP model is unavailable and no local tier committed.

use crate::taxonomy::ImageSubCategory;
use std::path::Path;

/// Blocking categorization via a remote vision API.
pub trait CloudCategorizer: Send + Sync {
    fn categorize_blocking(&self, path: &Path) -> Result<ImageSubCategory, String>;

    /// Short provider name for status messages.
    fn name(&self) -> &str {
        "cloud"
    }
}

/// Longest image side sent to cloud providers, in pixels.
pub const MAX_UPLOAD_SIDE: u32 = 768;

/// The instruction sent along with each image.
pub fn classification_prompt() -> String {
    let labels: Vec<String> = ImageSubCategory::ALL
        .iter()
        .map(|c| format!("{} ({})", c.label(), c.display_name()))
        .collect();
    format!(
        "Classify this image into exactly ONE of these categories:\n{}\n\n\
         Respond with only the category label in snake_case (for example: photo_landscape), \
         with no other words or punctuation.",
        labels.join("\n")
    )
}

/// Interpret a model's free-text answer as a category label.
///
/// Accepts minor formatting noise (quotes, trailing periods, spaces or hyphens
/// instead of underscores, a sentence containing the label).
pub fn parse_label(text: &str) -> Option<ImageSubCategory> {
    let cleaned: String = text
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == '.' || c == '*')
        .to_lowercase()
        .replace([' ', '-'], "_");
    if let Some(cat) = ImageSubCategory::from_label(&cleaned) {
        return Some(cat);
    }
    // Fall back to the longest label mentioned anywhere in the answer
    // (longest first so "photo_portrait" wins over "photo").
    let lower = text.to_lowercase();
    let mut labels: Vec<ImageSubCategory> = ImageSubCategory::ALL.to_vec();
    labels.sort_by_key(|c| std::cmp::Reverse(c.label().len()));
    labels.into_iter().find(|c| lower.contains(c.label()))
}

/// Standard base64 (RFC 4648) with padding.
pub fn base64_encode(data: &[u8]) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(4 * data.len().div_ceil(3));
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARSET[((n >> 18) & 0x3f) as usize] as char);
        result.push(CHARSET[((n >> 12) & 0x3f) as usize] as char);
        result.push(if chunk.len() > 1 {
            CHARSET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            CHARSET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    result
}

/// Decode, downscale and re-encode an image as JPEG for upload.
pub fn prepare_upload(path: &Path) -> Result<Vec<u8>, String> {
    let img = crate::open_image(path).map_err(|e| format!("cannot decode image: {e}"))?;
    let img = if img.width().max(img.height()) > MAX_UPLOAD_SIDE {
        img.resize(
            MAX_UPLOAD_SIDE,
            MAX_UPLOAD_SIDE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
    rgb.write_with_encoder(encoder)
        .map_err(|e| format!("cannot encode JPEG: {e}"))?;
    Ok(out)
}

/// Supported cloud vision providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Ollama,
    OpenAi,
    Gemini,
    Anthropic,
}

impl Provider {
    /// The model used when none is configured.
    pub fn default_model(&self) -> &'static str {
        match self {
            Self::Ollama => "llava",
            Self::OpenAi => "gpt-6-luna",
            Self::Gemini => "gemini-3.1-flash-lite",
            Self::Anthropic => "claude-opus-5-5",
        }
    }

    /// Environment variable consulted when no API key is configured.
    pub fn api_key_env(&self) -> Option<&'static str> {
        match self {
            Self::Ollama => None,
            Self::OpenAi => Some("OPENAI_API_KEY"),
            Self::Gemini => Some("GEMINI_API_KEY"),
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::OpenAi => "OpenAI",
            Self::Gemini => "Gemini",
            Self::Anthropic => "Anthropic",
        }
    }
}

/// HTTP vision categorizer for all supported providers (feature `cloud`).
#[cfg(feature = "cloud")]
pub struct HttpVisionCategorizer {
    provider: Provider,
    /// API base URL (only configurable for Ollama).
    base_url: String,
    api_key: String,
    model: String,
    client: reqwest::blocking::Client,
}

#[cfg(feature = "cloud")]
impl HttpVisionCategorizer {
    /// Create a categorizer. `model` empty means [`Provider::default_model`];
    /// `api_key` empty falls back to the provider's environment variable.
    pub fn new(
        provider: Provider,
        model: &str,
        api_key: &str,
        ollama_url: &str,
    ) -> Result<Self, String> {
        let mut key = api_key.trim().to_string();
        if key.is_empty() {
            if let Some(var) = provider.api_key_env() {
                key = std::env::var(var).unwrap_or_default();
            }
        }
        if provider != Provider::Ollama && key.is_empty() {
            return Err(format!(
                "{} needs an API key (set it in Settings or via {})",
                provider.display_name(),
                provider.api_key_env().unwrap_or("an environment variable")
            ));
        }
        let model = if model.trim().is_empty() {
            provider.default_model().to_string()
        } else {
            model.trim().to_string()
        };
        let base_url = match provider {
            Provider::Ollama => {
                let u = ollama_url.trim().trim_end_matches('/');
                if u.is_empty() {
                    "http://localhost:11434".to_string()
                } else {
                    u.to_string()
                }
            }
            Provider::OpenAi => "https://api.openai.com/v1".to_string(),
            Provider::Gemini => "https://generativelanguage.googleapis.com/v1beta".to_string(),
            Provider::Anthropic => "https://api.anthropic.com/v1".to_string(),
        };
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            provider,
            base_url,
            api_key: key,
            model,
            client,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn request(&self, jpeg_b64: &str, prompt: &str) -> reqwest::blocking::RequestBuilder {
        match self.provider {
            Provider::Ollama => self
                .client
                .post(format!("{}/api/generate", self.base_url))
                .json(&serde_json::json!({
                    "model": self.model,
                    "prompt": prompt,
                    "images": [jpeg_b64],
                    "stream": false,
                })),
            Provider::OpenAi => self
                .client
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&serde_json::json!({
                    "model": self.model,
                    "messages": [{
                        "role": "user",
                        "content": [
                            {"type": "text", "text": prompt},
                            {"type": "image_url", "image_url": {
                                "url": format!("data:image/jpeg;base64,{jpeg_b64}"),
                                "detail": "low",
                            }},
                        ],
                    }],
                    "max_completion_tokens": 300,
                })),
            Provider::Gemini => self
                .client
                .post(format!(
                    "{}/models/{}:generateContent",
                    self.base_url, self.model
                ))
                .header("x-goog-api-key", &self.api_key)
                .json(&serde_json::json!({
                    "contents": [{
                        "parts": [
                            {"inline_data": {"mime_type": "image/jpeg", "data": jpeg_b64}},
                            {"text": prompt},
                        ],
                    }],
                })),
            Provider::Anthropic => {
                let mut body = serde_json::json!({
                    "model": self.model,
                    "max_tokens": 1024,
                    "output_config": {"effort": "low"},
                    "messages": [{
                        "role": "user",
                        "content": [
                            {"type": "image", "source": {
                                "type": "base64",
                                "media_type": "image/jpeg",
                                "data": jpeg_b64,
                            }},
                            {"type": "text", "text": prompt},
                        ],
                    }],
                });
                let mut req = self
                    .client
                    .post(format!("{}/messages", self.base_url))
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01");
                // Let the API retry declined requests on a suitable model.
                if anthropic_supports_fallbacks(&self.model) {
                    body["fallbacks"] = serde_json::json!("default");
                    req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
                }
                req.json(&body)
            }
        }
    }

    fn extract_text(&self, json: &serde_json::Value) -> Result<String, String> {
        let text = match self.provider {
            Provider::Ollama => json["response"].as_str().map(String::from),
            Provider::OpenAi => json["choices"][0]["message"]["content"]
                .as_str()
                .map(String::from),
            Provider::Gemini => json["candidates"][0]["content"]["parts"]
                .as_array()
                .and_then(|parts| parts.iter().find_map(|p| p["text"].as_str()))
                .map(String::from),
            Provider::Anthropic => {
                if json["stop_reason"] == "refusal" {
                    return Err("the model declined to classify this image".into());
                }
                json["content"]
                    .as_array()
                    .and_then(|blocks| {
                        blocks
                            .iter()
                            .find(|b| b["type"] == "text")
                            .and_then(|b| b["text"].as_str())
                    })
                    .map(String::from)
            }
        };
        text.ok_or_else(|| "unexpected response format".to_string())
    }
}

/// Models that accept the `fallbacks: "default"` request field.
#[cfg(feature = "cloud")]
fn anthropic_supports_fallbacks(model: &str) -> bool {
    matches!(
        model,
        "claude-opus-5-5" | "claude-opus-5" | "claude-fable-5-1" | "claude-sonnet-5-5"
    )
}

#[cfg(feature = "cloud")]
impl CloudCategorizer for HttpVisionCategorizer {
    fn categorize_blocking(&self, path: &Path) -> Result<ImageSubCategory, String> {
        let name = self.provider.display_name();
        let jpeg = prepare_upload(path)?;
        let b64 = base64_encode(&jpeg);
        let resp = self
            .request(&b64, &classification_prompt())
            .send()
            .map_err(|e| format!("{name}: request failed: {e}"))?;
        let status = resp.status();
        let body = resp.text().map_err(|e| format!("{name}: {e}"))?;
        if !status.is_success() {
            let snippet: String = body.chars().take(300).collect();
            return Err(format!("{name}: HTTP {status}: {snippet}"));
        }
        let json: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("{name}: invalid JSON: {e}"))?;
        let text = self.extract_text(&json).map_err(|e| format!("{name}: {e}"))?;
        parse_label(&text).ok_or_else(|| format!("{name}: unknown category '{}'", text.trim()))
    }

    fn name(&self) -> &str {
        self.provider.display_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn parse_label_tolerates_noise() {
        assert_eq!(parse_label("meme"), Some(ImageSubCategory::Meme));
        assert_eq!(
            parse_label("  \"Photo Landscape.\" "),
            Some(ImageSubCategory::PhotoLandscape)
        );
        assert_eq!(
            parse_label("This looks like a photo_portrait to me"),
            Some(ImageSubCategory::PhotoPortrait)
        );
        assert_eq!(parse_label("no idea"), None);
    }

    #[test]
    fn prompt_lists_every_label() {
        let p = classification_prompt();
        for c in ImageSubCategory::ALL {
            assert!(p.contains(c.label()));
        }
    }

    #[test]
    fn upload_is_downscaled_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.png");
        image::RgbImage::from_pixel(2000, 1000, image::Rgb([10, 20, 30]))
            .save(&p)
            .unwrap();
        let jpeg = prepare_upload(&p).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!(decoded.width(), MAX_UPLOAD_SIDE);
        assert_eq!(decoded.height(), MAX_UPLOAD_SIDE / 2);
    }

    #[cfg(feature = "cloud")]
    #[test]
    fn api_providers_require_a_key() {
        // Only meaningful when the env var isn't set in the test environment.
        if std::env::var("OPENAI_API_KEY").is_err() {
            assert!(HttpVisionCategorizer::new(Provider::OpenAi, "", "", "").is_err());
        }
        let ollama = HttpVisionCategorizer::new(Provider::Ollama, "", "", "").unwrap();
        assert_eq!(ollama.model(), "llava");
    }
}
