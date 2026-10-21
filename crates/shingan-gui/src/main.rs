//! GUI entry point for shingan (心眼), built with [Iced](https://iced.rs).

mod app;
mod media;
mod platform;
mod tabs;
mod views;

fn main() -> iced::Result {
    iced::application(app::App::title, app::App::update, app::App::view)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window_size(iced::Size::new(1200.0, 820.0))
        .run_with(app::App::new)
}
