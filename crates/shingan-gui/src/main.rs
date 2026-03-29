//! GUI entry point for shingan (心眼), built with [Iced](https://iced.rs).
//!
//! Launches a 1200x800 desktop window for browsing, scanning, and managing
//! duplicate files interactively.

mod app;
mod tabs;
mod theme;
mod views;

fn main() -> iced::Result {
    iced::application(app::App::title, app::App::update, app::App::view)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window_size(iced::Size::new(1200.0, 800.0))
        .run_with(app::App::new)
}
