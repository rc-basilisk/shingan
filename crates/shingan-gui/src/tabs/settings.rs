use iced::widget::{
    button, checkbox, column, container, pick_list, progress_bar, row, scrollable, text,
    text_input, Rule,
};
use iced::{Element, Length, Task};
use shingan_ml::model_registry::{self, ModelDef, DEFAULT_MODELS};
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
    pub video_skip_secs: String,
    pub video_duration_secs: String,
    pub status_message: Option<String>,
    /// Per-model installed status, keyed by model ID.
    pub model_statuses: Vec<(String, bool)>,
    /// Current download state.
    pub download: DownloadState,
}

/// Download progress state shown in the UI.
#[derive(Debug, Clone)]
pub enum DownloadState {
    Idle,
    Downloading {
        model_id: String,
        current_file: String,
        file_index: usize,
        file_count: usize,
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    Failed {
        model_id: String,
        error: String,
    },
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
    VideoSkipSecsChanged(String),
    VideoDurationSecsChanged(String),
    StartDownload(String),
    CancelDownload,
    RemoveModel(String),
    DownloadProgress {
        current_file: String,
        file_index: usize,
        file_count: usize,
        downloaded: u64,
        total: u64,
    },
    DownloadComplete(Result<(), String>),
    SaveSettings,
    ClearSessions,
    OptimizeDb,
    ClearCache,
}

fn refresh_model_statuses(custom_path: &str) -> Vec<(String, bool)> {
    let dir = model_registry::resolve_models_dir(custom_path);
    DEFAULT_MODELS
        .iter()
        .map(|m| (m.id.to_string(), model_registry::model_installed(m, &dir)))
        .collect()
}

impl Default for SettingsState {
    fn default() -> Self {
        let settings = load_settings();
        let ml_path = settings.ml_model_path.clone().unwrap_or_default();
        let model_statuses = refresh_model_statuses(&ml_path);
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
            video_skip_secs: format!("{:.1}", settings.video_skip_secs),
            video_duration_secs: format!("{:.1}", settings.video_duration_secs),
            status_message: None,
            model_statuses,
            download: DownloadState::Idle,
        }
    }
}

impl SettingsState {
    /// Whether a download subscription should be active.
    pub fn is_downloading(&self) -> bool {
        matches!(self.download, DownloadState::Downloading { .. })
    }

    fn is_model_installed(&self, model_id: &str) -> bool {
        self.model_statuses
            .iter()
            .any(|(id, installed)| id == model_id && *installed)
    }

    pub fn update(&mut self, message: SettingsMessage) -> Task<SettingsMessage> {
        match message {
            SettingsMessage::ThreadCountChanged(val) => self.thread_count = val,
            SettingsMessage::CacheSizeChanged(val) => self.cache_size_mb = val,
            SettingsMessage::MlModelPathChanged(val) => {
                self.ml_model_path = val;
                self.model_statuses = refresh_model_statuses(&self.ml_model_path);
            }
            SettingsMessage::ConfidenceThresholdChanged(val) => self.confidence_threshold = val,
            SettingsMessage::ToggleCloud(val) => self.cloud_enabled = val,
            SettingsMessage::CloudProviderSelected(p) => self.cloud_provider = p,
            SettingsMessage::CloudApiKeyChanged(val) => self.cloud_api_key = val,
            SettingsMessage::MaxCloudRequestsChanged(val) => self.max_cloud_requests = val,
            SettingsMessage::OllamaUrlChanged(val) => self.ollama_url = val,
            SettingsMessage::VisionModelChanged(val) => self.vision_model = val,
            SettingsMessage::VideoSkipSecsChanged(val) => self.video_skip_secs = val,
            SettingsMessage::VideoDurationSecsChanged(val) => self.video_duration_secs = val,

            SettingsMessage::StartDownload(model_id) => {
                if let Some(model) = model_registry::find_model(&model_id) {
                    if !model_registry::model_downloadable(model) {
                        self.download = DownloadState::Failed {
                            model_id,
                            error: "Download URLs are not yet configured for this model."
                                .to_string(),
                        };
                    } else {
                        let first_file = model
                            .files
                            .first()
                            .map(|f| f.filename.to_string())
                            .unwrap_or_default();
                        self.download = DownloadState::Downloading {
                            model_id,
                            current_file: first_file,
                            file_index: 0,
                            file_count: model.files.len(),
                            downloaded_bytes: 0,
                            total_bytes: 0,
                        };
                        self.status_message = None;
                    }
                }
            }

            SettingsMessage::CancelDownload => {
                // Clean up partial files for the model being downloaded.
                if let DownloadState::Downloading { ref model_id, .. } = self.download {
                    if let Some(model) = model_registry::find_model(model_id) {
                        let dir = model_registry::resolve_models_dir(&self.ml_model_path);
                        for file in model.files {
                            let path = dir.join(file.filename);
                            // Only remove if it's not a complete, previously-installed file.
                            // We can't easily tell, so just leave files in place — the status
                            // check will reflect reality.
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
                self.download = DownloadState::Idle;
                self.model_statuses = refresh_model_statuses(&self.ml_model_path);
                self.status_message = Some("Download cancelled.".to_string());
            }

            SettingsMessage::RemoveModel(model_id) => {
                if let Some(model) = model_registry::find_model(&model_id) {
                    let dir = model_registry::resolve_models_dir(&self.ml_model_path);
                    match model_registry::remove_model(model, &dir) {
                        Ok(()) => {
                            self.status_message =
                                Some(format!("{} removed.", model.name));
                        }
                        Err(e) => {
                            self.status_message = Some(format!("Remove failed: {}", e));
                        }
                    }
                    self.model_statuses = refresh_model_statuses(&self.ml_model_path);
                }
            }

            SettingsMessage::DownloadProgress {
                current_file,
                file_index,
                file_count,
                downloaded,
                total,
            } => {
                if let DownloadState::Downloading {
                    current_file: ref mut cf,
                    file_index: ref mut fi,
                    file_count: ref mut fc,
                    downloaded_bytes: ref mut db,
                    total_bytes: ref mut tb,
                    ..
                } = self.download
                {
                    *cf = current_file;
                    *fi = file_index;
                    *fc = file_count;
                    *db = downloaded;
                    *tb = total;
                }
            }

            SettingsMessage::DownloadComplete(result) => {
                match result {
                    Ok(()) => {
                        self.status_message = Some("Model downloaded successfully.".to_string());
                        self.download = DownloadState::Idle;
                    }
                    Err(e) => {
                        let model_id = match &self.download {
                            DownloadState::Downloading { model_id, .. } => model_id.clone(),
                            _ => String::new(),
                        };
                        self.download = DownloadState::Failed {
                            model_id,
                            error: e,
                        };
                    }
                }
                self.model_statuses = refresh_model_statuses(&self.ml_model_path);
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
                    video_skip_secs: self.video_skip_secs.parse().unwrap_or(3.0),
                    video_duration_secs: self.video_duration_secs.parse().unwrap_or(20.0),
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
        let label_width = 220;
        let mut content = column![].spacing(16).padding([16, 24]);

        // -- Performance card --
        {
            let card = container(
                column![
                    text("Performance").size(16),
                    row![
                        text("Scanner threads:").width(label_width),
                        text_input("4", &self.thread_count)
                            .on_input(SettingsMessage::ThreadCountChanged)
                            .width(100),
                    ]
                    .spacing(10),
                    row![
                        text("Thumbnail cache size (MB):").width(label_width),
                        text_input("500", &self.cache_size_mb)
                            .on_input(SettingsMessage::CacheSizeChanged)
                            .width(100),
                    ]
                    .spacing(10),
                    row![
                        text("Video skip intro (secs):").width(label_width),
                        text_input("3.0", &self.video_skip_secs)
                            .on_input(SettingsMessage::VideoSkipSecsChanged)
                            .width(100),
                    ]
                    .spacing(10),
                    row![
                        text("Video sample duration (secs):").width(label_width),
                        text_input("20.0", &self.video_duration_secs)
                            .on_input(SettingsMessage::VideoDurationSecsChanged)
                            .width(100),
                    ]
                    .spacing(10),
                ]
                .spacing(10),
            )
            .padding(15)
            .width(Length::Fill);
            content = content.push(card);
        }

        content = content.push(Rule::horizontal(1));

        // -- Local ML card --
        {
            let mut ml_col = column![
                text("Local ML Categorization").size(16),
                row![
                    text("Confidence threshold (0-1):").width(label_width),
                    text_input("0.60", &self.confidence_threshold)
                        .on_input(SettingsMessage::ConfidenceThresholdChanged)
                        .width(100),
                ]
                .spacing(10),
                row![
                    text("Custom model path (optional):").width(label_width),
                    text_input("Leave empty for default", &self.ml_model_path)
                        .on_input(SettingsMessage::MlModelPathChanged)
                        .width(Length::Fill),
                ]
                .spacing(10),
            ]
            .spacing(10);

            // Model roster
            ml_col = ml_col.push(text("Models").size(14));

            for model in DEFAULT_MODELS {
                ml_col = ml_col.push(self.view_model_card(model));
            }

            let card = container(ml_col).padding(15).width(Length::Fill);
            content = content.push(card);
        }

        content = content.push(Rule::horizontal(1));

        // -- Cloud APIs card --
        {
            let mut cloud_fields = column![
                text("Cloud APIs (Advanced)").size(16),
                checkbox(
                    "Enable cloud escalation for low-confidence images",
                    self.cloud_enabled
                )
                .on_toggle(SettingsMessage::ToggleCloud),
                row![
                    text("Cloud provider:").width(label_width),
                    pick_list(
                        CloudProvider::ALL.as_slice(),
                        Some(self.cloud_provider),
                        SettingsMessage::CloudProviderSelected,
                    )
                    .width(200),
                ]
                .spacing(10),
            ]
            .spacing(10);

            if self.cloud_provider == CloudProvider::Ollama {
                cloud_fields = cloud_fields.push(
                    row![
                        text("Ollama API URL:").width(label_width),
                        text_input("http://localhost:11434", &self.ollama_url)
                            .on_input(SettingsMessage::OllamaUrlChanged)
                            .width(Length::Fill),
                    ]
                    .spacing(10),
                );
                cloud_fields = cloud_fields.push(
                    row![
                        text("Vision model:").width(label_width),
                        text_input("llava", &self.vision_model)
                            .on_input(SettingsMessage::VisionModelChanged)
                            .width(200),
                    ]
                    .spacing(10),
                );
            } else {
                cloud_fields = cloud_fields.push(
                    row![
                        text("API key:").width(label_width),
                        text_input("Enter API key...", &self.cloud_api_key)
                            .on_input(SettingsMessage::CloudApiKeyChanged)
                            .width(Length::Fill)
                            .secure(true),
                    ]
                    .spacing(10),
                );
            }

            cloud_fields = cloud_fields.push(
                row![
                    text("Max cloud requests/session:").width(label_width),
                    text_input("unlimited", &self.max_cloud_requests)
                        .on_input(SettingsMessage::MaxCloudRequestsChanged)
                        .width(100),
                ]
                .spacing(10),
            );

            let card = container(cloud_fields).padding(15).width(Length::Fill);
            content = content.push(card);
        }

        content = content.push(Rule::horizontal(1));

        // -- Database & Cache card --
        {
            let card = container(
                column![
                    text("Database & Cache").size(16),
                    row![
                        button(text("Clear Old Sessions").size(13))
                            .padding([6, 14])
                            .on_press(SettingsMessage::ClearSessions),
                        button(text("Optimize Database").size(13))
                            .padding([6, 14])
                            .on_press(SettingsMessage::OptimizeDb),
                        button(text("Clear Thumbnail Cache").size(13))
                            .padding([6, 14])
                            .on_press(SettingsMessage::ClearCache),
                    ]
                    .spacing(10),
                ]
                .spacing(10),
            )
            .padding(15)
            .width(Length::Fill);
            content = content.push(card);
        }

        content = content.push(Rule::horizontal(1));

        // -- Save --
        content = content.push(
            container(
                button(text("Save Settings").size(14))
                    .padding([10, 0])
                    .width(Length::Fill)
                    .on_press(SettingsMessage::SaveSettings),
            )
            .padding([4, 0]),
        );

        // -- Status --
        if let Some(ref msg) = self.status_message {
            content = content.push(container(text(msg).size(14)).padding([8, 12]));
        }

        scrollable(container(content).width(Length::Fill)).into()
    }

    /// Render a single model card within the ML section.
    fn view_model_card<'a>(&self, model: &ModelDef) -> Element<'a, SettingsMessage> {
        let installed = self.is_model_installed(model.id);

        // Header: name + size
        let header = row![
            text(model.name).size(13),
            iced::widget::horizontal_space(),
            text(model.size_label).size(12),
        ]
        .align_y(iced::Alignment::Center);

        let desc = text(model.description).size(12);

        // Status row depends on current state
        let status_row: Element<'a, SettingsMessage> = if let DownloadState::Downloading {
            ref model_id,
            ref current_file,
            file_index,
            file_count,
            downloaded_bytes,
            total_bytes,
            ..
        } = self.download
        {
            if model_id == model.id {
                let pct = if total_bytes > 0 {
                    (downloaded_bytes as f32 / total_bytes as f32) * 100.0
                } else {
                    0.0
                };
                let progress_label = if total_bytes > 0 {
                    format!(
                        "Downloading {} ({}/{})... {} / {}",
                        current_file,
                        file_index + 1,
                        file_count,
                        format_bytes(downloaded_bytes),
                        format_bytes(total_bytes),
                    )
                } else {
                    format!(
                        "Downloading {} ({}/{})... {}",
                        current_file,
                        file_index + 1,
                        file_count,
                        format_bytes(downloaded_bytes),
                    )
                };
                column![
                    text(progress_label).size(12),
                    row![
                        progress_bar(0.0..=100.0, pct).height(6).width(Length::Fill),
                        button(text("Cancel").size(11))
                            .padding([3, 8])
                            .on_press(SettingsMessage::CancelDownload),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                ]
                .spacing(4)
                .into()
            } else {
                // Another model is downloading — show this one's normal status.
                self.view_model_status_row(model, installed)
            }
        } else if let DownloadState::Failed {
            ref model_id,
            ref error,
        } = self.download
        {
            if model_id == model.id {
                column![
                    text(format!("Download failed: {}", error)).size(12),
                    row![
                        button(text("Retry").size(11))
                            .padding([3, 8])
                            .on_press(SettingsMessage::StartDownload(model.id.to_string())),
                    ],
                ]
                .spacing(4)
                .into()
            } else {
                self.view_model_status_row(model, installed)
            }
        } else {
            self.view_model_status_row(model, installed)
        };

        container(
            column![header, desc, status_row].spacing(4),
        )
        .padding([8, 12])
        .width(Length::Fill)
        .into()
    }

    /// The idle status row for a model: "Installed [Remove]" or "Not installed [Download]".
    fn view_model_status_row<'a>(
        &self,
        model: &ModelDef,
        installed: bool,
    ) -> Element<'a, SettingsMessage> {
        if installed {
            row![
                text("Installed").size(12),
                iced::widget::horizontal_space(),
                button(text("Remove").size(11))
                    .padding([3, 8])
                    .on_press(SettingsMessage::RemoveModel(model.id.to_string())),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            let downloadable = model_registry::model_downloadable(model);
            let mut dl_btn = button(text("Download").size(11)).padding([3, 8]);
            if downloadable && !self.is_downloading() {
                dl_btn = dl_btn.on_press(SettingsMessage::StartDownload(model.id.to_string()));
            }
            row![
                text("Not installed").size(12),
                iced::widget::horizontal_space(),
                dl_btn,
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into()
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.0} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{} B", bytes)
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
    /// Seconds to skip at the start of videos before sampling (default 3.0).
    #[serde(default = "default_video_skip_secs")]
    pub video_skip_secs: f64,
    /// Duration in seconds to sample from videos for hashing (default 20.0).
    #[serde(default = "default_video_duration_secs")]
    pub video_duration_secs: f64,
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
fn default_video_skip_secs() -> f64 {
    3.0
}
fn default_video_duration_secs() -> f64 {
    20.0
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
            video_skip_secs: default_video_skip_secs(),
            video_duration_secs: default_video_duration_secs(),
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
        assert!((s.video_skip_secs - 3.0).abs() < f64::EPSILON);
        assert!((s.video_duration_secs - 20.0).abs() < f64::EPSILON);
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
    fn settings_update_ml_model_path_refreshes_statuses() {
        let mut state = SettingsState::default();
        let _ = state.update(SettingsMessage::MlModelPathChanged(
            "/nonexistent/path".to_string(),
        ));
        assert_eq!(state.ml_model_path, "/nonexistent/path");
        // All models should show as not installed for a bogus path.
        for (_id, installed) in &state.model_statuses {
            assert!(!installed);
        }
    }

    #[test]
    fn settings_update_remove_nonexistent_model() {
        let mut state = SettingsState::default();
        // Removing from a nonexistent path should succeed (nothing to remove).
        state.ml_model_path = "/tmp/shingan_test_nonexistent".to_string();
        let _ = state.update(SettingsMessage::RemoveModel("clip-vit-b32".to_string()));
        assert!(state.status_message.is_some());
    }

    #[test]
    fn format_bytes_various_sizes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1_500), "2 KB");
        assert_eq!(format_bytes(5_500_000), "5.5 MB");
        assert_eq!(format_bytes(1_200_000_000), "1.2 GB");
    }
}
