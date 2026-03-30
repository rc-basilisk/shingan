//! Tier 3: optional cloud-based image categorization via remote vision APIs.
//!
//! This module defines the [`CloudCategorizer`] trait — a blocking interface for
//! sending an image to a remote vision-capable model and receiving back an
//! [`ImageSubCategory`] label. Concrete implementations are gated behind feature
//! flags:
//!
//! - `cloud-ollama` — `OllamaCategorizer`: sends base64-encoded images to a
//!   local Ollama server's `/api/generate` endpoint with a vision model (e.g.
//!   `llava`). Fully functional.
//! - `cloud-openai` — `OpenAiCategorizer`: placeholder for OpenAI's vision
//!   API. Currently returns an error; implement when API key flow is added.
//! - `cloud-gemini` — `GeminiCategorizer`: placeholder for Google Gemini's
//!   vision API. Currently returns an error; implement when API key flow is added.
//!
//! Cloud categorization is only invoked by the pipeline when local tiers (0–2)
//! produce confidence below [`PipelineConfig::cloud_escalation_threshold`](crate::pipeline::PipelineConfig::cloud_escalation_threshold),
//! or when the ONNX model is unavailable and no local tier committed.

use crate::taxonomy::ImageSubCategory;
use std::path::Path;

/// Async categorization via a remote vision API.
pub trait CloudCategorizer: Send + Sync {
    fn categorize_blocking(&self, path: &Path) -> Result<ImageSubCategory, String>;
}

/// Ollama `/api/generate` with vision (optional feature `cloud-ollama`).
#[cfg(feature = "cloud-ollama")]
pub struct OllamaCategorizer {
    /// Base URL of the Ollama server (e.g. `http://localhost:11434`).
    pub base_url: String,
    /// Vision model name (e.g. `llava`).
    pub model: String,
    /// HTTP client for blocking API calls.
    pub client: reqwest::blocking::Client,
}

#[cfg(feature = "cloud-ollama")]
impl OllamaCategorizer {
    pub fn new(base_url: &str, model: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            client: reqwest::blocking::Client::new(),
        }
    }

    fn categories_prompt() -> String {
        ImageSubCategory::ALL
            .iter()
            .map(|c| c.label())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(feature = "cloud-ollama")]
impl CloudCategorizer for OllamaCategorizer {
    fn categorize_blocking(&self, path: &Path) -> Result<ImageSubCategory, String> {
        let image_data = std::fs::read(path).map_err(|e| e.to_string())?;
        let b64 = base64_encode(&image_data);
        let prompt = format!(
            "Classify this image into exactly ONE of these categories: {}. Respond with only the category name, snake_case, nothing else.",
            Self::categories_prompt()
        );
        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "images": [b64],
            "stream": false,
        });
        let url = format!("{}/api/generate", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| e.to_string())?;
        let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
        let text = json["response"]
            .as_str()
            .ok_or_else(|| "missing response".to_string())?
            .trim()
            .to_lowercase();
        ImageSubCategory::from_label(&text).ok_or_else(|| "unknown category".to_string())
    }
}

#[cfg(feature = "cloud-ollama")]
fn base64_encode(data: &[u8]) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(4 * (data.len() / 3 + 1));
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARSET[((n >> 18) & 0x3f) as usize] as char);
        result.push(CHARSET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARSET[((n >> 6) & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARSET[(n & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Generic HTTP-based vision categorizer for cloud providers that use an API key
/// and accept base64-encoded images. Concrete providers configure the endpoint,
/// auth header, and response parser.
#[cfg(any(feature = "cloud-openai", feature = "cloud-gemini"))]
pub struct HttpVisionCategorizer {
    /// Display name for error messages (e.g. "OpenAI", "Gemini").
    pub provider_name: String,
    /// Full endpoint URL to POST to.
    pub endpoint: String,
    /// API key sent in the Authorization header.
    pub api_key: String,
    /// Vision model name (provider-specific).
    pub model: String,
    /// HTTP client.
    pub client: reqwest::blocking::Client,
    /// How to build the request body from (model, prompt, base64_image).
    pub build_body: fn(&str, &str, &str) -> serde_json::Value,
    /// How to extract the text response from the JSON response body.
    pub extract_response: fn(&serde_json::Value) -> Option<String>,
}

#[cfg(any(feature = "cloud-openai", feature = "cloud-gemini"))]
impl HttpVisionCategorizer {
    fn categories_prompt() -> String {
        ImageSubCategory::ALL
            .iter()
            .map(|c| c.label())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(any(feature = "cloud-openai", feature = "cloud-gemini"))]
impl CloudCategorizer for HttpVisionCategorizer {
    fn categorize_blocking(&self, path: &Path) -> Result<ImageSubCategory, String> {
        let image_data = std::fs::read(path).map_err(|e| e.to_string())?;
        let b64 = base64_encode_generic(&image_data);
        let prompt = format!(
            "Classify this image into exactly ONE of these categories: {}. Respond with only the category name, snake_case, nothing else.",
            Self::categories_prompt()
        );
        let body = (self.build_body)(&self.model, &prompt, &b64);
        let resp = self
            .client
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .map_err(|e| format!("{}: {}", self.provider_name, e))?;
        let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
        let text = (self.extract_response)(&json)
            .ok_or_else(|| format!("{}: unexpected response format", self.provider_name))?
            .trim()
            .to_lowercase();
        ImageSubCategory::from_label(&text)
            .ok_or_else(|| format!("{}: unknown category '{}'", self.provider_name, text))
    }
}

/// OpenAI vision categorizer (feature `cloud-openai`).
#[cfg(feature = "cloud-openai")]
pub fn openai_categorizer(api_key: &str, model: &str) -> HttpVisionCategorizer {
    HttpVisionCategorizer {
        provider_name: "OpenAI".to_string(),
        endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
        api_key: api_key.to_string(),
        model: model.to_string(),
        client: reqwest::blocking::Client::new(),
        build_body: |model, prompt, b64_image| {
            serde_json::json!({
                "model": model,
                "messages": [{
                    "role": "user",
                    "content": [
                        {"type": "text", "text": prompt},
                        {"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{}", b64_image)}},
                    ],
                }],
                "max_tokens": 50,
            })
        },
        extract_response: |json| {
            json["choices"][0]["message"]["content"].as_str().map(String::from)
        },
    }
}

/// Google Gemini vision categorizer (feature `cloud-gemini`).
#[cfg(feature = "cloud-gemini")]
pub fn gemini_categorizer(api_key: &str, model: &str) -> HttpVisionCategorizer {
    HttpVisionCategorizer {
        provider_name: "Gemini".to_string(),
        endpoint: format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model, api_key
        ),
        api_key: api_key.to_string(),
        model: model.to_string(),
        client: reqwest::blocking::Client::new(),
        build_body: |_model, prompt, b64_image| {
            serde_json::json!({
                "contents": [{
                    "parts": [
                        {"text": prompt},
                        {"inline_data": {"mime_type": "image/jpeg", "data": b64_image}},
                    ],
                }],
            })
        },
        extract_response: |json| {
            json["candidates"][0]["content"]["parts"][0]["text"].as_str().map(String::from)
        },
    }
}

#[cfg(any(feature = "cloud-openai", feature = "cloud-gemini"))]
fn base64_encode_generic(data: &[u8]) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(4 * (data.len() / 3 + 1));
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARSET[((n >> 18) & 0x3f) as usize] as char);
        result.push(CHARSET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARSET[((n >> 6) & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARSET[(n & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}
