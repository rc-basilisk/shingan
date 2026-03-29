use iced::Theme;

/// App theme state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AppTheme {
    #[default]
    Dark,
    Light,
}

impl AppTheme {
    pub fn toggle(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }

    pub fn to_iced_theme(self) -> Theme {
        match self {
            Self::Dark => Theme::Dark,
            Self::Light => Theme::Light,
        }
    }

    pub fn toggle_label(self) -> &'static str {
        match self {
            Self::Dark => "Light Mode",
            Self::Light => "Dark Mode",
        }
    }
}
