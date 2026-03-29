use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Element, Length, Task};
use std::path::PathBuf;

/// State for the Settings tab.
pub struct SettingsState {
    pub thread_count: String,
    pub cache_size_mb: String,
    pub ollama_url: String,
    pub vision_model: String,
    pub status_message: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    ThreadCountChanged(String),
    CacheSizeChanged(String),
    OllamaUrlChanged(String),
    VisionModelChanged(String),
    SaveSettings,
    ClearSessions,
    OptimizeDb,
    ClearCache,
}

impl Default for SettingsState {
    fn default() -> Self {
        let settings = load_settings();
        Self {
            thread_count: settings.thread_count.to_string(),
            cache_size_mb: settings.cache_size_mb.to_string(),
            ollama_url: settings.ollama_url,
            vision_model: settings.vision_model,
            status_message: None,
        }
    }
}

impl SettingsState {
    pub fn update(&mut self, message: SettingsMessage) -> Task<SettingsMessage> {
        match message {
            SettingsMessage::ThreadCountChanged(val) => self.thread_count = val,
            SettingsMessage::CacheSizeChanged(val) => self.cache_size_mb = val,
            SettingsMessage::OllamaUrlChanged(val) => self.ollama_url = val,
            SettingsMessage::VisionModelChanged(val) => self.vision_model = val,
            SettingsMessage::SaveSettings => {
                let settings = AppSettings {
                    thread_count: self.thread_count.parse().unwrap_or(4),
                    cache_size_mb: self.cache_size_mb.parse().unwrap_or(500),
                    ollama_url: self.ollama_url.clone(),
                    vision_model: self.vision_model.clone(),
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

        // Performance
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

        // ML settings
        content = content.push(text("ML Categorization (Ollama)").size(18));
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

        // Database maintenance
        content = content.push(text("Database").size(18));
        content = content.push(
            row![
                button("Clear Old Sessions").on_press(SettingsMessage::ClearSessions),
                button("Optimize Database").on_press(SettingsMessage::OptimizeDb),
            ]
            .spacing(10),
        );

        // Cache
        content = content.push(text("Cache").size(18));
        content = content.push(button("Clear Thumbnail Cache").on_press(SettingsMessage::ClearCache));

        // Save
        content = content.push(button("Save Settings").on_press(SettingsMessage::SaveSettings));

        // Status
        if let Some(ref msg) = self.status_message {
            content = content.push(text(msg));
        }

        scrollable(container(content).width(Length::Fill)).into()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub thread_count: u32,
    pub cache_size_mb: u32,
    pub ollama_url: String,
    pub vision_model: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            thread_count: 4,
            cache_size_mb: 500,
            ollama_url: "http://localhost:11434".to_string(),
            vision_model: "llava".to_string(),
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
