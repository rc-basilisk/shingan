use iced::widget::{
    button, checkbox, column, container, pick_list, progress_bar, row, scrollable, text, text_input,
};
use iced::{Element, Length, Task};
use shingan_ml::model_registry::{self, ModelDef, DEFAULT_MODELS};
use shingan_utils::settings::{AppSettings, CloudProvider};

/// State for the Settings tab. Text fields are kept as strings while editing
/// and parsed on save.
pub struct SettingsState {
    pub thread_count: String,
    pub min_file_size_kb: String,
    pub ml_model_path: String,
    pub confidence_threshold: String,
    pub cloud_enabled: bool,
    pub cloud_provider: CloudProvider,
    pub cloud_api_key: String,
    pub cloud_model: String,
    pub max_cloud_requests: String,
    pub ollama_url: String,
    pub vision_model: String,
    pub video_skip_secs: String,
    pub video_duration_secs: String,
    pub delete_permanently: bool,
    pub status_message: Option<String>,
    /// Per-model installed status, keyed by model ID.
    pub model_statuses: Vec<(String, bool)>,
    /// Current download state.
    pub download: DownloadState,
    /// Bumped on every download start so a retry restarts the subscription.
    pub download_generation: u64,
    /// Set by the app after a save; the app picks it up and applies it.
    pub saved: Option<AppSettings>,
    /// True when fields differ from what was last saved.
    pub dirty: bool,
    /// Requests for the app (database work).
    pub pending_action: Option<DbAction>,
}

/// Database actions the Settings tab asks the app to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbAction {
    ClearSessions,
    ClearSignatureCache,
    Optimize,
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

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    ThreadCountChanged(String),
    MinFileSizeChanged(String),
    MlModelPathChanged(String),
    ConfidenceThresholdChanged(String),
    ToggleCloud(bool),
    CloudProviderSelected(CloudProvider),
    CloudApiKeyChanged(String),
    CloudModelChanged(String),
    MaxCloudRequestsChanged(String),
    OllamaUrlChanged(String),
    VisionModelChanged(String),
    VideoSkipSecsChanged(String),
    VideoDurationSecsChanged(String),
    TogglePermanentDelete(bool),
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
    ResetDefaults,
    ClearSessions,
    ClearSignatureCache,
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

impl SettingsState {
    pub fn from_settings(settings: &AppSettings) -> Self {
        let ml_path = settings.ml_model_path.clone().unwrap_or_default();
        let model_statuses = refresh_model_statuses(&ml_path);
        Self {
            thread_count: settings.thread_count.to_string(),
            min_file_size_kb: settings.min_file_size_kb.to_string(),
            ml_model_path: ml_path,
            confidence_threshold: format!("{:.2}", settings.classification_confidence_threshold),
            cloud_enabled: settings.cloud_enabled,
            cloud_provider: settings.cloud_provider,
            cloud_api_key: settings.cloud_api_key.clone().unwrap_or_default(),
            cloud_model: settings.cloud_model.clone(),
            max_cloud_requests: settings
                .max_cloud_requests_per_session
                .map(|n| n.to_string())
                .unwrap_or_default(),
            ollama_url: settings.ollama_url.clone(),
            vision_model: settings.vision_model.clone(),
            video_skip_secs: format!("{:.1}", settings.video_skip_secs),
            video_duration_secs: format!("{:.1}", settings.video_duration_secs),
            delete_permanently: settings.delete_permanently,
            status_message: None,
            model_statuses,
            download: DownloadState::Idle,
            download_generation: 0,
            saved: None,
            dirty: false,
            pending_action: None,
        }
    }

    /// Whether a download subscription should be active.
    pub fn is_downloading(&self) -> bool {
        matches!(self.download, DownloadState::Downloading { .. })
    }

    fn is_model_installed(&self, model_id: &str) -> bool {
        self.model_statuses
            .iter()
            .any(|(id, installed)| id == model_id && *installed)
    }

    /// Whether the CLIP model is installed (for hints elsewhere in the UI).
    pub fn clip_installed(&self) -> bool {
        self.model_statuses.iter().any(|(_, installed)| *installed)
    }

    /// Build settings from the form, or explain what's invalid.
    fn to_settings(&self, base: &AppSettings) -> Result<AppSettings, String> {
        fn num<T: std::str::FromStr>(field: &str, value: &str) -> Result<T, String> {
            value
                .trim()
                .parse()
                .map_err(|_| format!("\"{value}\" is not a valid number for {field}"))
        }
        let confidence: f32 = num("confidence threshold", &self.confidence_threshold)?;
        if !(0.0..=1.0).contains(&confidence) {
            return Err("Confidence threshold must be between 0 and 1".into());
        }
        Ok(AppSettings {
            thread_count: num("scanner threads", &self.thread_count)?,
            min_file_size_kb: num("minimum file size", &self.min_file_size_kb)?,
            ollama_url: self.ollama_url.trim().to_string(),
            vision_model: self.vision_model.trim().to_string(),
            cloud_model: self.cloud_model.trim().to_string(),
            ml_model_path: Some(self.ml_model_path.trim().to_string()).filter(|s| !s.is_empty()),
            classification_confidence_threshold: confidence,
            cloud_enabled: self.cloud_enabled,
            cloud_provider: self.cloud_provider,
            cloud_api_key: Some(self.cloud_api_key.trim().to_string()).filter(|s| !s.is_empty()),
            max_cloud_requests_per_session: if self.max_cloud_requests.trim().is_empty() {
                None
            } else {
                Some(num("max cloud requests", &self.max_cloud_requests)?)
            },
            video_skip_secs: num("video skip", &self.video_skip_secs)?,
            video_duration_secs: num("video sample duration", &self.video_duration_secs)?,
            delete_permanently: self.delete_permanently,
            dark_theme: base.dark_theme,
        })
    }

    pub fn update(
        &mut self,
        message: SettingsMessage,
        current: &AppSettings,
    ) -> Task<SettingsMessage> {
        let edits = !matches!(
            message,
            SettingsMessage::StartDownload(_)
                | SettingsMessage::CancelDownload
                | SettingsMessage::RemoveModel(_)
                | SettingsMessage::DownloadProgress { .. }
                | SettingsMessage::DownloadComplete(_)
                | SettingsMessage::SaveSettings
                | SettingsMessage::ClearSessions
                | SettingsMessage::ClearSignatureCache
                | SettingsMessage::OptimizeDb
                | SettingsMessage::ClearCache
        );
        match message {
            SettingsMessage::ThreadCountChanged(val) => self.thread_count = val,
            SettingsMessage::MinFileSizeChanged(val) => self.min_file_size_kb = val,
            SettingsMessage::MlModelPathChanged(val) => {
                self.ml_model_path = val;
                self.model_statuses = refresh_model_statuses(&self.ml_model_path);
            }
            SettingsMessage::ConfidenceThresholdChanged(val) => self.confidence_threshold = val,
            SettingsMessage::ToggleCloud(val) => self.cloud_enabled = val,
            SettingsMessage::CloudProviderSelected(p) => {
                if p != self.cloud_provider {
                    self.cloud_model.clear();
                }
                self.cloud_provider = p;
            }
            SettingsMessage::CloudApiKeyChanged(val) => self.cloud_api_key = val,
            SettingsMessage::CloudModelChanged(val) => self.cloud_model = val,
            SettingsMessage::MaxCloudRequestsChanged(val) => self.max_cloud_requests = val,
            SettingsMessage::OllamaUrlChanged(val) => self.ollama_url = val,
            SettingsMessage::VisionModelChanged(val) => self.vision_model = val,
            SettingsMessage::VideoSkipSecsChanged(val) => self.video_skip_secs = val,
            SettingsMessage::VideoDurationSecsChanged(val) => self.video_duration_secs = val,
            SettingsMessage::TogglePermanentDelete(val) => self.delete_permanently = val,

            SettingsMessage::StartDownload(model_id) => {
                if let Some(model) = model_registry::find_model(&model_id) {
                    if !model_registry::model_downloadable(model) {
                        self.download = DownloadState::Failed {
                            model_id,
                            error: "No download URL is configured for this model.".to_string(),
                        };
                    } else {
                        self.download_generation += 1;
                        self.download = DownloadState::Downloading {
                            model_id,
                            current_file: model
                                .files
                                .first()
                                .map(|f| f.filename.to_string())
                                .unwrap_or_default(),
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
                // Only remove the partial download; an installed copy stays untouched.
                if let DownloadState::Downloading { ref model_id, .. } = self.download {
                    if let Some(model) = model_registry::find_model(model_id) {
                        let dir = model_registry::resolve_models_dir(&self.ml_model_path);
                        for file in model.files {
                            let part = model_registry::partial_path(&dir.join(file.filename));
                            let _ = std::fs::remove_file(part);
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
                    self.status_message = Some(match model_registry::remove_model(model, &dir) {
                        Ok(()) => format!("{} removed.", model.name),
                        Err(e) => format!("Remove failed: {}", e),
                    });
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
                        self.status_message = Some(
                            "Model downloaded. Image sub-categories are now available in Auto-Sort."
                                .to_string(),
                        );
                        self.download = DownloadState::Idle;
                    }
                    Err(e) => {
                        let model_id = match &self.download {
                            DownloadState::Downloading { model_id, .. } => model_id.clone(),
                            _ => String::new(),
                        };
                        self.download = DownloadState::Failed { model_id, error: e };
                    }
                }
                self.model_statuses = refresh_model_statuses(&self.ml_model_path);
            }

            SettingsMessage::SaveSettings => match self.to_settings(current) {
                Ok(settings) => match shingan_utils::settings::save_settings(&settings) {
                    Ok(()) => {
                        self.status_message = Some("Settings saved.".to_string());
                        self.saved = Some(settings);
                        self.dirty = false;
                    }
                    Err(e) => self.status_message = Some(format!("Could not save settings: {e}")),
                },
                Err(e) => self.status_message = Some(e),
            },
            SettingsMessage::ResetDefaults => {
                let keep_theme = AppSettings {
                    dark_theme: current.dark_theme,
                    ..AppSettings::default()
                };
                let mut fresh = Self::from_settings(&keep_theme);
                fresh.download = std::mem::replace(&mut self.download, DownloadState::Idle);
                fresh.download_generation = self.download_generation;
                *self = fresh;
                self.status_message =
                    Some("Defaults restored — press Save to keep them.".to_string());
            }
            SettingsMessage::ClearSessions => self.pending_action = Some(DbAction::ClearSessions),
            SettingsMessage::ClearSignatureCache => {
                self.pending_action = Some(DbAction::ClearSignatureCache)
            }
            SettingsMessage::OptimizeDb => self.pending_action = Some(DbAction::Optimize),
            SettingsMessage::ClearCache => {
                let base = dirs::cache_dir()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("shingan");
                let mut freed = 0u64;
                for sub in ["thumbnails", "previews"] {
                    let dir = base.join(sub);
                    freed += dir_size(&dir);
                    let _ = std::fs::remove_dir_all(&dir);
                }
                self.status_message = Some(format!(
                    "Preview cache cleared ({} freed).",
                    format_bytes(freed)
                ));
            }
        }
        if edits {
            self.dirty = true;
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, SettingsMessage> {
        let label_width = 240;
        let field = |label: &'static str, input: Element<'static, SettingsMessage>| {
            row![text(label).width(label_width), input]
                .spacing(10)
                .align_y(iced::Alignment::Center)
        };
        let hint = |s: &'static str| text(s).size(12).style(text::secondary);
        let mut content = column![].spacing(16).padding([16, 24]);

        // -- Scanning --
        content = content.push(card(
            column![
                text("Scanning").size(16),
                field(
                    "Worker threads (0 = all cores):",
                    text_input("0", &self.thread_count)
                        .on_input(SettingsMessage::ThreadCountChanged)
                        .width(100)
                        .into(),
                ),
                field(
                    "Ignore files smaller than (KiB):",
                    text_input("1", &self.min_file_size_kb)
                        .on_input(SettingsMessage::MinFileSizeChanged)
                        .width(100)
                        .into(),
                ),
                field(
                    "Video: skip first (seconds):",
                    text_input("3.0", &self.video_skip_secs)
                        .on_input(SettingsMessage::VideoSkipSecsChanged)
                        .width(100)
                        .into(),
                ),
                field(
                    "Video: sample length (seconds):",
                    text_input("20.0", &self.video_duration_secs)
                        .on_input(SettingsMessage::VideoDurationSecsChanged)
                        .width(100)
                        .into(),
                ),
                hint("Skipping intros helps match re-encoded copies of the same video."),
                checkbox(
                    "Delete duplicates permanently (instead of moving them to the trash)",
                    self.delete_permanently,
                )
                .on_toggle(SettingsMessage::TogglePermanentDelete),
            ]
            .spacing(10),
        ));

        // -- Local ML --
        let mut ml_col = column![
            text("Image sub-categories (local AI)").size(16),
            hint("Lets Auto-Sort file images into folders like photo_landscape or screenshot_desktop. Runs entirely on this computer."),
            field(
                "Minimum confidence (0-1):",
                text_input("0.20", &self.confidence_threshold)
                    .on_input(SettingsMessage::ConfidenceThresholdChanged)
                    .width(100)
                    .into(),
            ),
            hint("Images the model is less sure about go to images/other."),
            field(
                "Model folder (optional):",
                text_input("Default location", &self.ml_model_path)
                    .on_input(SettingsMessage::MlModelPathChanged)
                    .width(Length::Fill)
                    .into(),
            ),
        ]
        .spacing(10);
        for model in DEFAULT_MODELS {
            ml_col = ml_col.push(self.view_model_card(model));
        }
        content = content.push(card(ml_col));

        // -- Cloud --
        let mut cloud_fields = column![
            text("Cloud classification (optional)").size(16),
            hint("Ask an online (or local Ollama) vision model about images the local model is unsure of. Images are downscaled to 768 px before sending."),
            checkbox("Enable cloud classification", self.cloud_enabled)
                .on_toggle(SettingsMessage::ToggleCloud),
            field(
                "Provider:",
                pick_list(
                    CloudProvider::ALL.as_slice(),
                    Some(self.cloud_provider),
                    SettingsMessage::CloudProviderSelected,
                )
                .width(220)
                .into(),
            ),
        ]
        .spacing(10);

        if self.cloud_provider == CloudProvider::Ollama {
            cloud_fields = cloud_fields
                .push(field(
                    "Ollama URL:",
                    text_input("http://localhost:11434", &self.ollama_url)
                        .on_input(SettingsMessage::OllamaUrlChanged)
                        .width(Length::Fill)
                        .into(),
                ))
                .push(field(
                    "Vision model:",
                    text_input("llava", &self.vision_model)
                        .on_input(SettingsMessage::VisionModelChanged)
                        .width(220)
                        .into(),
                ));
        } else {
            let env = self
                .cloud_provider
                .provider()
                .api_key_env()
                .unwrap_or_default();
            cloud_fields = cloud_fields
                .push(field(
                    "Model:",
                    text_input(self.cloud_provider.default_model(), &self.cloud_model)
                        .on_input(SettingsMessage::CloudModelChanged)
                        .width(260)
                        .into(),
                ))
                .push(field(
                    "API key:",
                    text_input("Leave empty to use the environment variable", &self.cloud_api_key)
                        .on_input(SettingsMessage::CloudApiKeyChanged)
                        .width(Length::Fill)
                        .secure(true)
                        .into(),
                ))
                .push(
                    text(format!(
                        "Keys are saved unencrypted in settings.json. Setting {env} instead keeps them out of the file."
                    ))
                    .size(12)
                    .style(text::secondary),
                );
        }
        cloud_fields = cloud_fields.push(field(
            "Max requests per sort:",
            text_input("unlimited", &self.max_cloud_requests)
                .on_input(SettingsMessage::MaxCloudRequestsChanged)
                .width(100)
                .into(),
        ));
        content = content.push(card(cloud_fields));

        // -- Database & cache --
        content = content.push(card(
            column![
                text("Data").size(16),
                row![
                    button(text("Delete scan history").size(13))
                        .padding([6, 14])
                        .style(button::danger)
                        .on_press(SettingsMessage::ClearSessions),
                    button(text("Forget cached fingerprints").size(13))
                        .padding([6, 14])
                        .style(button::secondary)
                        .on_press(SettingsMessage::ClearSignatureCache),
                    button(text("Compact database").size(13))
                        .padding([6, 14])
                        .style(button::secondary)
                        .on_press(SettingsMessage::OptimizeDb),
                    button(text("Clear preview cache").size(13))
                        .padding([6, 14])
                        .style(button::secondary)
                        .on_press(SettingsMessage::ClearCache),
                ]
                .spacing(10)
                .wrap(),
                hint("Cached fingerprints make re-scans of unchanged files instant; forgetting them only costs time."),
            ]
            .spacing(10),
        ));

        // -- Save --
        let save_label = if self.dirty {
            "Save settings (unsaved changes)"
        } else {
            "Save settings"
        };
        content = content.push(
            row![
                button(text(save_label).size(14))
                    .padding([10, 20])
                    .style(button::primary)
                    .on_press(SettingsMessage::SaveSettings),
                button(text("Reset to defaults").size(13))
                    .padding([10, 16])
                    .style(button::secondary)
                    .on_press(SettingsMessage::ResetDefaults),
            ]
            .spacing(10),
        );

        if let Some(ref msg) = self.status_message {
            content = content.push(container(text(msg).size(14)).padding([8, 12]));
        }

        scrollable(container(content).width(Length::Fill)).into()
    }

    /// Render a single model card within the ML section.
    fn view_model_card<'a>(&self, model: &ModelDef) -> Element<'a, SettingsMessage> {
        let installed = self.is_model_installed(model.id);

        let header = row![
            text(model.name).size(13),
            iced::widget::horizontal_space(),
            text(model.size_label).size(12),
        ]
        .align_y(iced::Alignment::Center);
        let desc = text(model.description).size(12);

        let status_row: Element<'a, SettingsMessage> = match &self.download {
            DownloadState::Downloading {
                model_id,
                current_file,
                file_index,
                file_count,
                downloaded_bytes,
                total_bytes,
            } if model_id == model.id => {
                let pct = if *total_bytes > 0 {
                    (*downloaded_bytes as f32 / *total_bytes as f32) * 100.0
                } else {
                    0.0
                };
                let mut label = format!(
                    "Downloading {} ({}/{})... {}",
                    current_file,
                    file_index + 1,
                    file_count,
                    format_bytes(*downloaded_bytes)
                );
                if *total_bytes > 0 {
                    label.push_str(&format!(" / {}", format_bytes(*total_bytes)));
                }
                column![
                    text(label).size(12),
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
            }
            DownloadState::Failed { model_id, error } if model_id == model.id => column![
                text(format!("Download failed: {}", error)).size(12),
                button(text("Retry").size(11))
                    .padding([3, 8])
                    .on_press(SettingsMessage::StartDownload(model.id.to_string())),
            ]
            .spacing(4)
            .into(),
            _ => self.view_model_status_row(model, installed),
        };

        container(column![header, desc, status_row].spacing(4))
            .padding([8, 12])
            .width(Length::Fill)
            .style(container::rounded_box)
            .into()
    }

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
                    .style(button::secondary)
                    .on_press(SettingsMessage::RemoveModel(model.id.to_string())),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            let mut dl_btn = button(text("Download").size(11)).padding([3, 8]);
            if model_registry::model_downloadable(model) && !self.is_downloading() {
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

fn card<'a>(content: impl Into<Element<'a, SettingsMessage>>) -> Element<'a, SettingsMessage> {
    container(content)
        .padding(15)
        .width(Length::Fill)
        .style(container::bordered_box)
        .into()
}

fn dir_size(dir: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

pub fn format_bytes(bytes: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> SettingsState {
        SettingsState::from_settings(&AppSettings::default())
    }

    #[test]
    fn edits_mark_dirty_and_invalid_numbers_are_reported() {
        let mut s = state();
        let base = AppSettings::default();
        let _ = s.update(SettingsMessage::ThreadCountChanged("abc".into()), &base);
        assert!(s.dirty);
        assert!(s.to_settings(&base).is_err());
        let _ = s.update(SettingsMessage::ThreadCountChanged("8".into()), &base);
        assert_eq!(s.to_settings(&base).unwrap().thread_count, 8);
    }

    #[test]
    fn confidence_must_be_in_range() {
        let mut s = state();
        s.confidence_threshold = "1.5".into();
        assert!(s.to_settings(&AppSettings::default()).is_err());
    }

    #[test]
    fn switching_provider_clears_custom_model() {
        let mut s = state();
        let base = AppSettings::default();
        let _ = s.update(
            SettingsMessage::CloudProviderSelected(CloudProvider::OpenAI),
            &base,
        );
        let _ = s.update(SettingsMessage::CloudModelChanged("custom".into()), &base);
        let _ = s.update(
            SettingsMessage::CloudProviderSelected(CloudProvider::Gemini),
            &base,
        );
        assert!(s.cloud_model.is_empty());
    }

    #[test]
    fn db_buttons_become_pending_actions() {
        let mut s = state();
        let _ = s.update(SettingsMessage::ClearSessions, &AppSettings::default());
        assert_eq!(s.pending_action, Some(DbAction::ClearSessions));
        assert!(!s.dirty);
    }

    #[test]
    fn format_bytes_various_sizes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1_500), "2 KB");
        assert_eq!(format_bytes(5_500_000), "5.5 MB");
        assert_eq!(format_bytes(1_200_000_000), "1.2 GB");
    }
}
