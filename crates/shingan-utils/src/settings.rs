//! Persisted user settings shared by the GUI and the CLI.
//!
//! Stored as JSON at `<config dir>/shingan/settings.json`
//! (`~/.config/shingan/settings.json` on Linux,
//! `~/Library/Application Support/shingan/settings.json` on macOS,
//! `%APPDATA%\shingan\settings.json` on Windows). Unknown fields are ignored and
//! missing fields get defaults, so files from older versions keep working.

use serde::{Deserialize, Serialize};
use shingan_ml::cloud::Provider;
use shingan_ml::{CloudCategorizer, PipelineConfig};
use std::path::PathBuf;
use std::sync::Arc;

/// Which cloud backend to use when cloud escalation is enabled.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudProvider {
    #[default]
    Ollama,
    OpenAI,
    Gemini,
    Anthropic,
}

impl CloudProvider {
    pub const ALL: [CloudProvider; 4] = [
        CloudProvider::Ollama,
        CloudProvider::OpenAI,
        CloudProvider::Gemini,
        CloudProvider::Anthropic,
    ];

    pub fn provider(&self) -> Provider {
        match self {
            Self::Ollama => Provider::Ollama,
            Self::OpenAI => Provider::OpenAi,
            Self::Gemini => Provider::Gemini,
            Self::Anthropic => Provider::Anthropic,
        }
    }

    pub fn default_model(&self) -> &'static str {
        self.provider().default_model()
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "ollama" => Some(Self::Ollama),
            "openai" => Some(Self::OpenAI),
            "gemini" | "google" => Some(Self::Gemini),
            "anthropic" | "claude" => Some(Self::Anthropic),
            _ => None,
        }
    }
}

impl std::fmt::Display for CloudProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ollama => write!(f, "Ollama (local)"),
            Self::OpenAI => write!(f, "OpenAI"),
            Self::Gemini => write!(f, "Google Gemini"),
            Self::Anthropic => write!(f, "Anthropic Claude"),
        }
    }
}

/// Persisted application settings (JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Worker threads for scanning (0 = one per CPU core).
    #[serde(default)]
    pub thread_count: u32,
    /// Files smaller than this (KiB) are ignored by the duplicate finder.
    #[serde(default = "default_min_file_size_kb")]
    pub min_file_size_kb: u64,
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
    /// Model used with Ollama.
    #[serde(default = "default_vision_model")]
    pub vision_model: String,
    /// Model used with OpenAI / Gemini / Anthropic (empty = provider default).
    #[serde(default)]
    pub cloud_model: String,
    #[serde(default)]
    pub ml_model_path: Option<String>,
    /// Images classified with less confidence than this go to `images/other`.
    #[serde(default = "default_confidence_threshold")]
    pub classification_confidence_threshold: f32,
    #[serde(default)]
    pub cloud_enabled: bool,
    #[serde(default)]
    pub cloud_provider: CloudProvider,
    /// Stored in plain text; prefer the provider's environment variable.
    #[serde(default)]
    pub cloud_api_key: Option<String>,
    #[serde(default)]
    pub max_cloud_requests_per_session: Option<u32>,
    /// Seconds to skip at the start of videos before sampling (default 3.0).
    #[serde(default = "default_video_skip_secs")]
    pub video_skip_secs: f64,
    /// Duration in seconds to sample from videos for hashing (default 20.0).
    #[serde(default = "default_video_duration_secs")]
    pub video_duration_secs: f64,
    /// Delete duplicates permanently instead of moving them to the trash.
    #[serde(default)]
    pub delete_permanently: bool,
    /// GUI theme: true = dark.
    #[serde(default = "default_true")]
    pub dark_theme: bool,
}

fn default_true() -> bool {
    true
}
fn default_min_file_size_kb() -> u64 {
    1
}
fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}
fn default_vision_model() -> String {
    "llava".to_string()
}
fn default_confidence_threshold() -> f32 {
    0.20
}
fn default_video_skip_secs() -> f64 {
    3.0
}
fn default_video_duration_secs() -> f64 {
    20.0
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            thread_count: 0,
            min_file_size_kb: default_min_file_size_kb(),
            ollama_url: default_ollama_url(),
            vision_model: default_vision_model(),
            cloud_model: String::new(),
            ml_model_path: None,
            classification_confidence_threshold: default_confidence_threshold(),
            cloud_enabled: false,
            cloud_provider: CloudProvider::default(),
            cloud_api_key: None,
            max_cloud_requests_per_session: None,
            video_skip_secs: default_video_skip_secs(),
            video_duration_secs: default_video_duration_secs(),
            delete_permanently: false,
            dark_theme: true,
        }
    }
}

impl AppSettings {
    /// Pipeline configuration derived from these settings.
    pub fn pipeline_config(&self) -> PipelineConfig {
        PipelineConfig {
            min_confidence: self.classification_confidence_threshold.clamp(0.0, 1.0),
            max_cloud_requests: self.max_cloud_requests_per_session,
            model_dir: self
                .ml_model_path
                .as_ref()
                .filter(|p| !p.trim().is_empty())
                .map(PathBuf::from),
            ..PipelineConfig::default()
        }
    }

    /// The effective model name for the selected cloud provider.
    pub fn effective_cloud_model(&self) -> String {
        match self.cloud_provider {
            CloudProvider::Ollama => self.vision_model.clone(),
            _ if self.cloud_model.trim().is_empty() => {
                self.cloud_provider.default_model().to_string()
            }
            _ => self.cloud_model.trim().to_string(),
        }
    }

    /// Build the cloud categorizer if cloud escalation is enabled.
    ///
    /// Returns `Ok(None)` when disabled and `Err` when enabled but misconfigured
    /// (e.g. missing API key).
    pub fn cloud_categorizer(&self) -> Result<Option<Arc<dyn CloudCategorizer>>, String> {
        if !self.cloud_enabled {
            return Ok(None);
        }
        let c = shingan_ml::cloud::HttpVisionCategorizer::new(
            self.cloud_provider.provider(),
            &self.effective_cloud_model(),
            self.cloud_api_key.as_deref().unwrap_or(""),
            &self.ollama_url,
        )?;
        Ok(Some(Arc::new(c)))
    }
}

/// Location of `settings.json`.
pub fn settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("shingan")
        .join("settings.json")
}

/// Load settings, falling back to defaults if the file is missing or invalid.
pub fn load_settings() -> AppSettings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Save settings. On Unix the file is made readable by the owner only, since
/// it may contain an API key.
pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let s = AppSettings::default();
        assert_eq!(s.thread_count, 0);
        assert!(!s.cloud_enabled);
        assert!(!s.delete_permanently);
        assert_eq!(s.cloud_provider, CloudProvider::Ollama);
    }

    #[test]
    fn old_settings_files_still_load() {
        // v0.1.x file with a field that no longer exists.
        let json = r#"{"thread_count":4,"cache_size_mb":500,"vision_model":"llava:13b"}"#;
        let s: AppSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.thread_count, 4);
        assert_eq!(s.vision_model, "llava:13b");
        assert!(s.dark_theme);
    }

    #[test]
    fn effective_model_defaults_per_provider() {
        let mut s = AppSettings {
            cloud_provider: CloudProvider::Gemini,
            ..Default::default()
        };
        assert_eq!(s.effective_cloud_model(), "gemini-3.1-flash-lite");
        s.cloud_model = "gemini-custom".into();
        assert_eq!(s.effective_cloud_model(), "gemini-custom");
        s.cloud_provider = CloudProvider::Ollama;
        assert_eq!(s.effective_cloud_model(), "llava");
    }

    #[test]
    fn disabled_cloud_builds_nothing() {
        assert!(AppSettings::default()
            .cloud_categorizer()
            .unwrap()
            .is_none());
    }

    #[test]
    fn provider_parse() {
        assert_eq!(
            CloudProvider::parse("Claude"),
            Some(CloudProvider::Anthropic)
        );
        assert_eq!(CloudProvider::parse("nope"), None);
    }
}
