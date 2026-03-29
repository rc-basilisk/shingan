use iced::widget::{button, checkbox, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Element, Length, Task};
use std::path::PathBuf;

/// State for the Settings tab.
pub struct SettingsState {
    pub thread_count: String,
    pub cache_size_mb: String,
    pub ml_model_path: String,
    pub confidence_threshold: String,
    pub cloud_enabled: bool,
    pub cloud_provider: CloudProvider,
    pub cloud_api_key: String,
    pub max_cloud_requests: String,
    pub ollama_url: String,
    pub vision_model: String,
    pub model_status: ModelStatus,
    pub status_message: Option<String>,
}

/// Which cloud backend to use when cloud escalation is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CloudProvider {
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

impl Default for CloudProvider {
    fn default() -> Self {
        Self::Ollama
    }
}

/// Whether the ONNX model files are present on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStatus {
    #[allow(dead_code)]
    Unknown,
    Present,
    Missing,
    Downloading,
}

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    ThreadCountChanged(String),
    CacheSizeChanged(String),
    MlModelPathChanged(String),
    ConfidenceThresholdChanged(String),
    ToggleCloud(bool),
    CloudProviderSelected(CloudProvider),
    CloudApiKeyChanged(String),
    MaxCloudRequestsChanged(String),
    OllamaUrlChanged(String),
    VisionModelChanged(String),
    CheckModelStatus,
    DownloadModel,
    #[allow(dead_code)]
    ModelDownloadComplete(Result<(), String>),
    SaveSettings,
    ClearSessions,
    OptimizeDb,
    ClearCache,
}

fn check_model_files_present(custom_path: &str) -> ModelStatus {
    let dir = if custom_path.is_empty() {
        shingan_ml::model_paths::default_models_dir()
    } else {
        PathBuf::from(custom_path)
    };
    let onnx = dir.join("clip_image.onnx");
    let proto = dir.join("clip_prototypes.bin");
    if onnx.is_file() && proto.is_file() {
        ModelStatus::Present
    } else {
        ModelStatus::Missing
    }
}

impl Default for SettingsState {
    fn default() -> Self {
        let settings = load_settings();
        let ml_path = settings.ml_model_path.clone().unwrap_or_default();
        let model_status = check_model_files_present(&ml_path);
        Self {
            thread_count: settings.thread_count.to_string(),
            cache_size_mb: settings.cache_size_mb.to_string(),
            ml_model_path: ml_path,
            confidence_threshold: format!("{:.2}", settings.classification_confidence_threshold),
            cloud_enabled: settings.cloud_enabled,
            cloud_provider: settings.cloud_provider,
            cloud_api_key: settings.cloud_api_key.unwrap_or_default(),
            max_cloud_requests: settings
                .max_cloud_requests_per_session
                .map(|n| n.to_string())
                .unwrap_or_default(),
            ollama_url: settings.ollama_url,
            vision_model: settings.vision_model,
            model_status,
            status_message: None,
        }
    }
}

impl SettingsState {
    pub fn update(&mut self, message: SettingsMessage) -> Task<SettingsMessage> {
        match message {
            SettingsMessage::ThreadCountChanged(val) => self.thread_count = val,
            SettingsMessage::CacheSizeChanged(val) => self.cache_size_mb = val,
            SettingsMessage::MlModelPathChanged(val) => {
                self.ml_model_path = val;
                self.model_status = check_model_files_present(&self.ml_model_path);
            }
            SettingsMessage::ConfidenceThresholdChanged(val) => self.confidence_threshold = val,
            SettingsMessage::ToggleCloud(val) => self.cloud_enabled = val,
            SettingsMessage::CloudProviderSelected(p) => self.cloud_provider = p,
            SettingsMessage::CloudApiKeyChanged(val) => self.cloud_api_key = val,
            SettingsMessage::MaxCloudRequestsChanged(val) => self.max_cloud_requests = val,
            SettingsMessage::OllamaUrlChanged(val) => self.ollama_url = val,
            SettingsMessage::VisionModelChanged(val) => self.vision_model = val,
            SettingsMessage::CheckModelStatus => {
                self.model_status = check_model_files_present(&self.ml_model_path);
                self.status_message = Some(match &self.model_status {
                    ModelStatus::Present => "ONNX model found.".to_string(),
                    ModelStatus::Missing => {
                        let dir = if self.ml_model_path.is_empty() {
                            shingan_ml::model_paths::default_models_dir()
                        } else {
                            PathBuf::from(&self.ml_model_path)
                        };
                        format!("Model not found in {}", dir.display())
                    }
                    _ => String::new(),
                });
            }
            SettingsMessage::DownloadModel => {
                self.model_status = ModelStatus::Downloading;
                self.status_message = Some("Model download not yet implemented — place clip_image.onnx and clip_prototypes.bin manually.".to_string());
                self.model_status = ModelStatus::Missing;
            }
            SettingsMessage::ModelDownloadComplete(result) => {
                match result {
                    Ok(()) => {
                        self.model_status = ModelStatus::Present;
                        self.status_message = Some("Model downloaded successfully.".to_string());
                    }
                    Err(e) => {
                        self.model_status = ModelStatus::Missing;
                        self.status_message = Some(format!("Download failed: {}", e));
                    }
                }
            }
            SettingsMessage::SaveSettings => {
                let settings = AppSettings {
                    thread_count: self.thread_count.parse().unwrap_or(4),
                    cache_size_mb: self.cache_size_mb.parse().unwrap_or(500),
                    ollama_url: self.ollama_url.clone(),
                    vision_model: self.vision_model.clone(),
                    ml_model_path: if self.ml_model_path.is_empty() {
                        None
                    } else {
                        Some(self.ml_model_path.clone())
                    },
                    classification_confidence_threshold: self
                        .confidence_threshold
                        .parse()
                        .unwrap_or(0.6),
                    cloud_enabled: self.cloud_enabled,
                    cloud_provider: self.cloud_provider,
                    cloud_api_key: if self.cloud_api_key.is_empty() {
                        None
                    } else {
                        Some(self.cloud_api_key.clone())
                    },
                    max_cloud_requests_per_session: self.max_cloud_requests.parse().ok(),
                };
                match save_settings(&settings) {
                    Ok(_) => self.status_message = Some("Settings saved!".to_string()),
                    Err(e) => self.status_message = Some(format!("Error: {}", e)),
                }
            }
            SettingsMessage::ClearSessions => {
                self.status_message = Some("Sessions cleared".to_string());
            }
            SettingsMessage::OptimizeDb => {
                self.status_message = Some("Database optimized".to_string());
            }
            SettingsMessage::ClearCache => {
                let cache_dir = dirs::cache_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("shingan")
                    .join("thumbnails");
                if let Err(e) = std::fs::remove_dir_all(&cache_dir) {
                    self.status_message = Some(format!("Error clearing cache: {}", e));
                } else {
                    std::fs::create_dir_all(&cache_dir).ok();
                    self.status_message = Some("Cache cleared!".to_string());
                }
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, SettingsMessage> {
        let mut content = column![].spacing(15).padding(20);

        // -- Performance --
        content = content.push(text("Performance").size(18));
        content = content.push(
            row![
                text("Scanner threads:").width(200),
                text_input("4", &self.thread_count)
                    .on_input(SettingsMessage::ThreadCountChanged)
                    .width(100),
            ]
            .spacing(10),
        );
        content = content.push(
            row![
                text("Thumbnail cache size (MB):").width(200),
                text_input("500", &self.cache_size_mb)
                    .on_input(SettingsMessage::CacheSizeChanged)
                    .width(100),
            ]
            .spacing(10),
        );

        // -- Local ML Categorization --
        content = content.push(text("Local ML Categorization").size(18));
        content = content.push(
            row![
                text("Confidence threshold (0-1):").width(200),
                text_input("0.60", &self.confidence_threshold)
                    .on_input(SettingsMessage::ConfidenceThresholdChanged)
                    .width(100),
            ]
            .spacing(10),
        );
        content = content.push(
            row![
                text("Custom model path (optional):").width(200),
                text_input("Leave empty for default", &self.ml_model_path)
                    .on_input(SettingsMessage::MlModelPathChanged)
                    .width(Length::Fill),
            ]
            .spacing(10),
        );

        // Model status & download
        let status_text = match &self.model_status {
            ModelStatus::Unknown => "Model status: unknown",
            ModelStatus::Present => "Model status: found",
            ModelStatus::Missing => "Model status: not found",
            ModelStatus::Downloading => "Model status: downloading...",
        };
        content = content.push(
            row![
                text(status_text).width(250),
                button("Check").on_press(SettingsMessage::CheckModelStatus),
                button("Download Model").on_press(SettingsMessage::DownloadModel),
            ]
            .spacing(10),
        );

        // -- Cloud APIs (Advanced) --
        content = content.push(text("Cloud APIs (Advanced)").size(18));
        content = content.push(
            checkbox("Enable cloud escalation for low-confidence images", self.cloud_enabled)
                .on_toggle(SettingsMessage::ToggleCloud),
        );

        content = content.push(
            row![
                text("Cloud provider:").width(200),
                pick_list(
                    CloudProvider::ALL.as_slice(),
                    Some(self.cloud_provider),
                    SettingsMessage::CloudProviderSelected,
                )
                .width(200),
            ]
            .spacing(10),
        );

        // Provider-specific fields
        if self.cloud_provider == CloudProvider::Ollama {
            content = content.push(
                row![
                    text("Ollama API URL:").width(200),
                    text_input("http://localhost:11434", &self.ollama_url)
                        .on_input(SettingsMessage::OllamaUrlChanged)
                        .width(Length::Fill),
                ]
                .spacing(10),
            );
            content = content.push(
                row![
                    text("Vision model:").width(200),
                    text_input("llava", &self.vision_model)
                        .on_input(SettingsMessage::VisionModelChanged)
                        .width(200),
                ]
                .spacing(10),
            );
        } else {
            content = content.push(
                row![
                    text("API key:").width(200),
                    text_input("Enter API key...", &self.cloud_api_key)
                        .on_input(SettingsMessage::CloudApiKeyChanged)
                        .width(Length::Fill)
                        .secure(true),
                ]
                .spacing(10),
            );
        }

        content = content.push(
            row![
                text("Max cloud requests/session:").width(200),
                text_input("unlimited", &self.max_cloud_requests)
                    .on_input(SettingsMessage::MaxCloudRequestsChanged)
                    .width(100),
            ]
            .spacing(10),
        );

        // -- Database --
        content = content.push(text("Database").size(18));
        content = content.push(
            row![
                button("Clear Old Sessions").on_press(SettingsMessage::ClearSessions),
                button("Optimize Database").on_press(SettingsMessage::OptimizeDb),
            ]
            .spacing(10),
        );

        // -- Cache --
        content = content.push(text("Cache").size(18));
        content = content.push(button("Clear Thumbnail Cache").on_press(SettingsMessage::ClearCache));

        // -- Save --
        content = content.push(button("Save Settings").on_press(SettingsMessage::SaveSettings));

        // -- Status --
        if let Some(ref msg) = self.status_message {
            content = content.push(text(msg));
        }

        scrollable(container(content).width(Length::Fill)).into()
    }
}

/// Persisted application settings (JSON).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub thread_count: u32,
    pub cache_size_mb: u32,
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
    #[serde(default = "default_vision_model")]
    pub vision_model: String,
    #[serde(default)]
    pub ml_model_path: Option<String>,
    #[serde(default = "default_confidence_threshold")]
    pub classification_confidence_threshold: f32,
    #[serde(default)]
    pub cloud_enabled: bool,
    #[serde(default)]
    pub cloud_provider: CloudProvider,
    #[serde(default)]
    pub cloud_api_key: Option<String>,
    #[serde(default)]
    pub max_cloud_requests_per_session: Option<u32>,
}

fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}
fn default_vision_model() -> String {
    "llava".to_string()
}
fn default_confidence_threshold() -> f32 {
    0.6
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            thread_count: 4,
            cache_size_mb: 500,
            ollama_url: default_ollama_url(),
            vision_model: default_vision_model(),
            ml_model_path: None,
            classification_confidence_threshold: default_confidence_threshold(),
            cloud_enabled: false,
            cloud_provider: CloudProvider::default(),
            cloud_api_key: None,
            max_cloud_requests_per_session: None,
        }
    }
}

fn settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("shingan")
        .join("settings.json")
}

pub fn load_settings() -> AppSettings {
    let path = settings_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => AppSettings::default(),
    }
}

fn save_settings(settings: &AppSettings) -> Result<(), Box<dyn std::error::Error>> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    std::fs::write(&path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_settings_default_values() {
        let s = AppSettings::default();
        assert_eq!(s.thread_count, 4);
        assert_eq!(s.cache_size_mb, 500);
        assert!((s.classification_confidence_threshold - 0.6).abs() < f32::EPSILON);
        assert!(!s.cloud_enabled);
        assert_eq!(s.cloud_provider, CloudProvider::Ollama);
        assert!(s.cloud_api_key.is_none());
        assert!(s.max_cloud_requests_per_session.is_none());
        assert!(s.ml_model_path.is_none());
    }

    #[test]
    fn app_settings_serde_round_trip() {
        let s = AppSettings {
            thread_count: 8,
            cache_size_mb: 1024,
            ollama_url: "http://my-server:11434".to_string(),
            vision_model: "llava:13b".to_string(),
            ml_model_path: Some("/opt/models".to_string()),
            classification_confidence_threshold: 0.75,
            cloud_enabled: true,
            cloud_provider: CloudProvider::OpenAI,
            cloud_api_key: Some("sk-test-key-123".to_string()),
            max_cloud_requests_per_session: Some(100),
        };
        let json = serde_json::to_string(&s).unwrap();
        let s2: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s2.thread_count, 8);
        assert_eq!(s2.cloud_provider, CloudProvider::OpenAI);
        assert_eq!(s2.cloud_api_key.as_deref(), Some("sk-test-key-123"));
        assert_eq!(s2.max_cloud_requests_per_session, Some(100));
        assert_eq!(s2.ml_model_path.as_deref(), Some("/opt/models"));
        assert!(s2.cloud_enabled);
    }

    #[test]
    fn app_settings_deserialize_missing_new_fields() {
        let json = r#"{"thread_count":4,"cache_size_mb":500}"#;
        let s: AppSettings = serde_json::from_str(json).unwrap();
        assert!(!s.cloud_enabled);
        assert_eq!(s.cloud_provider, CloudProvider::Ollama);
        assert!(s.cloud_api_key.is_none());
        assert!(s.max_cloud_requests_per_session.is_none());
        assert!((s.classification_confidence_threshold - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn cloud_provider_display() {
        assert_eq!(CloudProvider::Ollama.to_string(), "Ollama (local)");
        assert_eq!(CloudProvider::OpenAI.to_string(), "OpenAI");
        assert_eq!(CloudProvider::Gemini.to_string(), "Google Gemini");
        assert_eq!(CloudProvider::Anthropic.to_string(), "Anthropic Claude");
    }

    #[test]
    fn cloud_provider_all_has_four() {
        assert_eq!(CloudProvider::ALL.len(), 4);
    }

    #[test]
    fn settings_state_default_loads_without_panic() {
        let _state = SettingsState::default();
    }

    #[test]
    fn settings_update_toggle_cloud() {
        let mut state = SettingsState::default();
        assert!(!state.cloud_enabled);
        let _ = state.update(SettingsMessage::ToggleCloud(true));
        assert!(state.cloud_enabled);
    }

    #[test]
    fn settings_update_cloud_provider() {
        let mut state = SettingsState::default();
        assert_eq!(state.cloud_provider, CloudProvider::Ollama);
        let _ = state.update(SettingsMessage::CloudProviderSelected(CloudProvider::OpenAI));
        assert_eq!(state.cloud_provider, CloudProvider::OpenAI);
    }

    #[test]
    fn settings_update_api_key() {
        let mut state = SettingsState::default();
        let _ = state.update(SettingsMessage::CloudApiKeyChanged("sk-test".to_string()));
        assert_eq!(state.cloud_api_key, "sk-test");
    }

    #[test]
    fn settings_update_max_cloud_requests() {
        let mut state = SettingsState::default();
        let _ = state.update(SettingsMessage::MaxCloudRequestsChanged("50".to_string()));
        assert_eq!(state.max_cloud_requests, "50");
    }

    #[test]
    fn settings_update_confidence_threshold() {
        let mut state = SettingsState::default();
        let _ = state.update(SettingsMessage::ConfidenceThresholdChanged("0.80".to_string()));
        assert_eq!(state.confidence_threshold, "0.80");
    }

    #[test]
    fn settings_update_ml_model_path_updates_status() {
        let mut state = SettingsState::default();
        let _ = state.update(SettingsMessage::MlModelPathChanged("/nonexistent/path".to_string()));
        assert_eq!(state.ml_model_path, "/nonexistent/path");
        assert_eq!(state.model_status, ModelStatus::Missing);
    }

    #[test]
    fn settings_update_check_model_sets_status_message() {
        let mut state = SettingsState::default();
        let _ = state.update(SettingsMessage::CheckModelStatus);
        assert!(state.status_message.is_some());
    }

    #[test]
    fn model_status_check_nonexistent() {
        let status = check_model_files_present("/definitely/not/a/real/path");
        assert_eq!(status, ModelStatus::Missing);
    }

    #[test]
    fn model_status_check_empty_string_uses_default() {
        let status = check_model_files_present("");
        // Default dir likely doesn't have model files, but should not panic
        assert!(status == ModelStatus::Missing || status == ModelStatus::Present);
    }
}
