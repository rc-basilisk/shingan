use crate::tabs::duplicate_finder::{video_thumbnail_path, FinderMessage, PreviewFile};
use crate::views::file_preview::format_size;
use iced::widget::{
    button, checkbox, column, container, horizontal_rule, image, rich_text, row, scrollable, text,
};
use iced::{Color, Element, Length};
use shingan_core::file_info::FileCategory;
use shingan_core::scanner::grouping::DuplicateGroup;
use std::collections::HashMap;
use std::path::Path;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

const THUMB_SIZE: f32 = 140.0;

/// Render the results viewer overlay.
pub fn view<'a>(
    groups: &'a HashMap<FileCategory, Vec<DuplicateGroup>>,
    filter: &Option<FileCategory>,
    selected: &std::collections::HashSet<String>,
    deletion_message: &'a Option<String>,
    preview: &'a Option<PreviewFile>,
    page: usize,
    page_size: usize,
) -> Element<'a, FinderMessage> {
    if let Some(pf) = preview {
        return render_preview_modal(pf);
    }
    render_results_list(groups, filter, selected, deletion_message, page, page_size)
}

// ── Results list ────────────────────────────────────────────

fn render_results_list<'a>(
    groups: &'a HashMap<FileCategory, Vec<DuplicateGroup>>,
    filter: &Option<FileCategory>,
    selected: &std::collections::HashSet<String>,
    deletion_message: &'a Option<String>,
    page: usize,
    page_size: usize,
) -> Element<'a, FinderMessage> {
    let mut content = column![].spacing(12).padding(16);

    let total_groups: usize = groups.values().map(|g| g.len()).sum();
    let total_files: usize = groups
        .values()
        .flat_map(|g| g.iter())
        .map(|g| g.files.len())
        .sum();

    content = content.push(
        row![
            text(format!(
                "Found {} duplicate groups ({} files)",
                total_groups, total_files
            ))
            .size(20),
            iced::widget::horizontal_space(),
            button(text("Close").size(14)).on_press(FinderMessage::CloseResults),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    );
    content = content.push(horizontal_rule(1));

    let filters: Vec<(Option<FileCategory>, &str)> = vec![
        (None, "All"),
        (Some(FileCategory::Image), "Images"),
        (Some(FileCategory::Document), "Documents"),
        (Some(FileCategory::Video), "Videos"),
        (Some(FileCategory::Archive), "Archives"),
        (Some(FileCategory::Code), "Code"),
    ];
    let mut filter_row = row![].spacing(4);
    for (cat, label) in &filters {
        let is_active = filter == cat;
        let btn = button(text(*label).size(13)).on_press(FinderMessage::FilterChanged(*cat));
        filter_row = filter_row.push(if is_active {
            btn.style(button::primary)
        } else {
            btn.style(button::secondary)
        });
    }
    content = content.push(filter_row);

    content = content.push(
        row![
            button(text("Select All").size(13)).on_press(FinderMessage::SelectAll),
            button(text("Select None").size(13)).on_press(FinderMessage::SelectNone),
            button(text("Keep Newest").size(13)).on_press(FinderMessage::KeepNewest),
            button(text("Keep Largest").size(13)).on_press(FinderMessage::KeepLargest),
        ]
        .spacing(4),
    );

    if let Some(msg) = deletion_message {
        content = content.push(container(text(msg.as_str()).size(14)).padding(8));
    }

    let visible: Vec<(FileCategory, &DuplicateGroup)> = groups
        .iter()
        .filter(|(c, _)| filter.is_none_or(|f| *c == &f))
        .flat_map(|(cat, grp)| grp.iter().map(move |g| (*cat, g)))
        .collect();

    let total_visible = visible.len();
    let last_page = if page_size > 0 {
        total_visible.saturating_sub(1) / page_size
    } else {
        0
    };
    let current_page = page.min(last_page);
    let page_items = visible
        .into_iter()
        .skip(current_page * page_size)
        .take(page_size);

    let mut groups_content = column![].spacing(16);
    let mut group_index = (current_page * page_size) as u32;
    for (category, group) in page_items {
        group_index += 1;
        groups_content = groups_content.push(render_group(group_index, category, group, selected));
    }
    content = content.push(scrollable(groups_content).height(Length::Fill));

    let first_btn = if current_page > 0 {
        button(text("First").size(13)).on_press(FinderMessage::PageFirst)
    } else {
        button(text("First").size(13)).style(button::secondary)
    };
    let prev_btn = if current_page > 0 {
        button(text("Prev").size(13)).on_press(FinderMessage::PagePrev)
    } else {
        button(text("Prev").size(13)).style(button::secondary)
    };
    let next_btn = if current_page < last_page {
        button(text("Next").size(13)).on_press(FinderMessage::PageNext)
    } else {
        button(text("Next").size(13)).style(button::secondary)
    };
    let last_btn = if current_page < last_page {
        button(text("Last").size(13)).on_press(FinderMessage::PageLast)
    } else {
        button(text("Last").size(13)).style(button::secondary)
    };
    let page_label = format!(
        "Page {} of {} ({} total groups)",
        current_page + 1,
        last_page + 1,
        total_visible,
    );

    content = content.push(horizontal_rule(1));
    content = content.push(
        row![
            first_btn,
            prev_btn,
            text(page_label).size(13),
            next_btn,
            last_btn
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    );

    content = content.push(horizontal_rule(1));
    let delete_btn = if !selected.is_empty() {
        button(text("Delete Selected").size(14))
            .on_press(FinderMessage::DeleteSelected)
            .style(button::danger)
    } else {
        button(text("Delete Selected").size(14)).style(button::secondary)
    };
    content = content.push(
        row![
            text(format!("{} files selected for deletion", selected.len())).size(14),
            iced::widget::horizontal_space(),
            button(text("Export CSV").size(14)).on_press(FinderMessage::ExportResults),
            delete_btn,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    );

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(4)
        .into()
}

// ── Group card ──────────────────────────────────────────────

fn render_group<'a>(
    index: u32,
    category: FileCategory,
    group: &'a DuplicateGroup,
    selected: &std::collections::HashSet<String>,
) -> Element<'a, FinderMessage> {
    let mut group_col = column![].spacing(8);
    group_col = group_col.push(
        text(format!(
            "Group {} - {} - {:.1}% similar - {} files",
            index,
            category.label(),
            group.similarity * 100.0,
            group.files.len()
        ))
        .size(14),
    );

    let mut files_row = row![].spacing(12);
    for file in &group.files {
        files_row = files_row.push(render_file_card(file, category, selected));
    }

    group_col = group_col.push(
        scrollable(files_row).direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::default(),
        )),
    );

    container(group_col)
        .padding(12)
        .width(Length::Fill)
        .style(container::bordered_box)
        .into()
}

// ── File card ───────────────────────────────────────────────

fn file_type_icon(category: FileCategory, ext: &str) -> (String, Color) {
    match category {
        FileCategory::Image => ("IMG".to_string(), Color::from_rgb(0.3, 0.7, 0.9)),
        FileCategory::Video => ("VID".to_string(), Color::from_rgb(0.9, 0.4, 0.4)),
        FileCategory::Document => {
            let (icon, color) = match ext {
                "pdf" => ("PDF", Color::from_rgb(0.9, 0.2, 0.2)),
                "doc" | "docx" => ("DOC", Color::from_rgb(0.2, 0.4, 0.9)),
                "odt" => ("ODT", Color::from_rgb(0.2, 0.6, 0.9)),
                "txt" => ("TXT", Color::from_rgb(0.7, 0.7, 0.7)),
                "rtf" => ("RTF", Color::from_rgb(0.5, 0.5, 0.8)),
                _ => ("DOC", Color::from_rgb(0.5, 0.5, 0.7)),
            };
            (icon.to_string(), color)
        }
        FileCategory::Code => {
            let (icon, color) = match ext {
                "rs" => ("RS", Color::from_rgb(0.9, 0.5, 0.2)),
                "py" => ("PY", Color::from_rgb(0.3, 0.6, 0.9)),
                "js" => ("JS", Color::from_rgb(0.95, 0.85, 0.3)),
                "ts" | "tsx" => ("TS", Color::from_rgb(0.2, 0.5, 0.9)),
                "go" => ("GO", Color::from_rgb(0.0, 0.7, 0.8)),
                "c" | "cpp" | "h" => ("C", Color::from_rgb(0.4, 0.5, 0.9)),
                "html" => ("HTM", Color::from_rgb(0.9, 0.4, 0.2)),
                "css" => ("CSS", Color::from_rgb(0.3, 0.5, 0.9)),
                "jsx" => ("JSX", Color::from_rgb(0.4, 0.8, 0.9)),
                "vue" => ("VUE", Color::from_rgb(0.3, 0.8, 0.4)),
                "exs" => ("EX", Color::from_rgb(0.5, 0.3, 0.7)),
                _ => ("CODE", Color::from_rgb(0.6, 0.8, 0.3)),
            };
            (icon.to_string(), color)
        }
        FileCategory::Archive => {
            let (icon, color) = match ext {
                "zip" => ("ZIP", Color::from_rgb(0.9, 0.7, 0.2)),
                "tar" | "gz" | "bz2" | "xz" | "zst" => ("TAR", Color::from_rgb(0.7, 0.5, 0.2)),
                "7z" => ("7Z", Color::from_rgb(0.5, 0.7, 0.3)),
                "rar" => ("RAR", Color::from_rgb(0.6, 0.3, 0.7)),
                _ => ("ARC", Color::from_rgb(0.6, 0.6, 0.4)),
            };
            (icon.to_string(), color)
        }
    }
}

fn render_file_card<'a>(
    file_path: &'a Path,
    category: FileCategory,
    selected: &std::collections::HashSet<String>,
) -> Element<'a, FinderMessage> {
    let path_str = file_path.to_string_lossy().to_string();
    let is_selected = selected.contains(&path_str);
    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path_str.clone());
    let ext = file_path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let parent_dir = file_path
        .parent()
        .map(|p| {
            let s = p.to_string_lossy();
            if s.len() > 40 {
                format!("...{}", &s[s.len() - 40..])
            } else {
                s.to_string()
            }
        })
        .unwrap_or_default();
    let size = std::fs::metadata(file_path)
        .map(|m| format_size(m.len()))
        .unwrap_or_else(|_| "?".to_string());

    let card_width = THUMB_SIZE + 20.0;
    let preview_path = path_str.clone();

    // Build the thumbnail area
    let thumb_area: Element<'a, FinderMessage> = match category {
        FileCategory::Image => {
            let handle = image::Handle::from_path(file_path);
            button(
                image(handle)
                    .width(Length::Fixed(THUMB_SIZE))
                    .height(Length::Fixed(THUMB_SIZE)),
            )
            .on_press(FinderMessage::PreviewFile(preview_path, category))
            .style(button::text)
            .padding(2)
            .into()
        }
        FileCategory::Video => {
            // Try to show a cached video thumbnail
            if let Some(thumb_path) = video_thumbnail_path(file_path) {
                let handle = image::Handle::from_path(thumb_path);
                button(
                    image(handle)
                        .width(Length::Fixed(THUMB_SIZE))
                        .height(Length::Fixed(THUMB_SIZE)),
                )
                .on_press(FinderMessage::PreviewFile(preview_path, category))
                .style(button::text)
                .padding(2)
                .into()
            } else {
                let (icon, color) = file_type_icon(category, &ext);
                button(
                    column![text(icon).size(22).color(color), text("Preview").size(10),]
                        .spacing(4)
                        .width(Length::Fixed(THUMB_SIZE))
                        .height(Length::Fixed(THUMB_SIZE))
                        .align_x(iced::Alignment::Center),
                )
                .on_press(FinderMessage::PreviewFile(preview_path, category))
                .style(button::secondary)
                .padding(2)
                .into()
            }
        }
        _ => {
            let (icon, color) = file_type_icon(category, &ext);
            let can_preview = matches!(category, FileCategory::Document | FileCategory::Code);
            let btn_content = column![
                text(icon).size(22).color(color),
                text(ext.to_uppercase()).size(10),
            ]
            .spacing(4)
            .width(Length::Fixed(THUMB_SIZE))
            .height(Length::Fixed(THUMB_SIZE))
            .align_x(iced::Alignment::Center);

            if can_preview {
                button(btn_content)
                    .on_press(FinderMessage::PreviewFile(preview_path, category))
                    .style(button::secondary)
                    .padding(2)
                    .into()
            } else {
                container(btn_content)
                    .style(container::bordered_box)
                    .center_x(Length::Fixed(THUMB_SIZE))
                    .center_y(Length::Fixed(THUMB_SIZE))
                    .into()
            }
        }
    };

    let name_display = if file_name.len() > 22 {
        format!("{}...", &file_name[..19])
    } else {
        file_name
    };
    let explorer_path = path_str.clone();
    let path_for_toggle = path_str;

    let card = column![
        thumb_area,
        text(name_display).size(13),
        button(text(parent_dir).size(10))
            .on_press(FinderMessage::OpenInExplorer(explorer_path))
            .style(button::text)
            .padding(0),
        text(size).size(11),
        checkbox("Delete", is_selected)
            .on_toggle(move |_| FinderMessage::ToggleFileForDeletion(path_for_toggle.clone()))
            .size(16)
            .text_size(12),
    ]
    .spacing(4)
    .width(Length::Fixed(card_width));

    container(card).padding(8).into()
}

// ── Preview modal ───────────────────────────────────────────

fn render_preview_modal<'a>(pf: &'a PreviewFile) -> Element<'a, FinderMessage> {
    let file_path = Path::new(&pf.path);
    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| pf.path.clone());
    let size = std::fs::metadata(file_path)
        .map(|m| format_size(m.len()))
        .unwrap_or_else(|_| "?".to_string());

    let mut content = column![].spacing(10).padding(16);

    // Header
    content = content.push(
        row![
            text(format!("{} ({})", file_name, size)).size(18),
            iced::widget::horizontal_space(),
            button(text("Open with system").size(12))
                .on_press(FinderMessage::OpenWithSystem(pf.path.clone()))
                .style(button::secondary),
            button(text("Open folder").size(12))
                .on_press(FinderMessage::OpenInExplorer(pf.path.clone()))
                .style(button::secondary),
            button(text("Back").size(14)).on_press(FinderMessage::ClosePreview),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    );
    content = content.push(text(pf.path.clone()).size(11));
    content = content.push(horizontal_rule(1));

    // Preview content
    match pf.category {
        FileCategory::Image => {
            // Zoom controls
            content = content.push(
                row![
                    button(text("-").size(16)).on_press(FinderMessage::ZoomOut),
                    text(format!("{:.0}%", pf.zoom * 100.0)).size(14),
                    button(text("+").size(16)).on_press(FinderMessage::ZoomIn),
                    button(text("Reset").size(12)).on_press(FinderMessage::ZoomReset),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );

            let handle = image::Handle::from_path(file_path);
            // Get natural image dimensions for zoom
            let (w, h) = get_image_dimensions(file_path).unwrap_or((800, 600));
            let zoomed_w = (w as f32 * pf.zoom).max(100.0);
            let zoomed_h = (h as f32 * pf.zoom).max(75.0);

            content = content.push(
                scrollable(
                    container(
                        image(handle)
                            .width(Length::Fixed(zoomed_w))
                            .height(Length::Fixed(zoomed_h)),
                    )
                    .center_x(Length::Fill)
                    .padding(8),
                )
                .height(Length::Fill),
            );
        }
        FileCategory::Video => {
            // Show the extracted frame if available
            if let Some(ref rendered) = pf.rendered_image {
                // Zoom controls
                content = content.push(
                    row![
                        button(text("-").size(16)).on_press(FinderMessage::ZoomOut),
                        text(format!("{:.0}%", pf.zoom * 100.0)).size(14),
                        button(text("+").size(16)).on_press(FinderMessage::ZoomIn),
                        button(text("Reset").size(12)).on_press(FinderMessage::ZoomReset),
                        iced::widget::horizontal_space(),
                        button(text("Open in video player").size(13))
                            .on_press(FinderMessage::OpenWithSystem(pf.path.clone())),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                );

                let handle = image::Handle::from_path(Path::new(rendered));
                let (w, h) = get_image_dimensions(Path::new(rendered)).unwrap_or((800, 450));
                let zoomed_w = (w as f32 * pf.zoom).max(100.0);
                let zoomed_h = (h as f32 * pf.zoom).max(75.0);

                content = content.push(
                    scrollable(
                        container(
                            image(handle)
                                .width(Length::Fixed(zoomed_w))
                                .height(Length::Fixed(zoomed_h)),
                        )
                        .center_x(Length::Fill)
                        .padding(8),
                    )
                    .height(Length::Fill),
                );
            } else {
                content = content.push(
                    column![
                        text("Video frame extraction failed or ffmpeg not available").size(14),
                        button(text("Open in video player").size(14))
                            .on_press(FinderMessage::OpenWithSystem(pf.path.clone())),
                    ]
                    .spacing(10),
                );
            }
        }
        FileCategory::Document if pf.path.ends_with(".pdf") => {
            // PDF page navigation
            content = content.push(
                row![
                    button(text("< Prev").size(13)).on_press(FinderMessage::PdfPrevPage),
                    text(format!("Page {}", pf.pdf_page + 1)).size(14),
                    button(text("Next >").size(13)).on_press(FinderMessage::PdfNextPage),
                    iced::widget::horizontal_space(),
                    button(text("-").size(16)).on_press(FinderMessage::ZoomOut),
                    text(format!("{:.0}%", pf.zoom * 100.0)).size(14),
                    button(text("+").size(16)).on_press(FinderMessage::ZoomIn),
                    button(text("Reset").size(12)).on_press(FinderMessage::ZoomReset),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );

            if let Some(ref rendered) = pf.rendered_image {
                let handle = image::Handle::from_path(Path::new(rendered));
                let (w, h) = get_image_dimensions(Path::new(rendered)).unwrap_or((800, 1100));
                let zoomed_w = (w as f32 * pf.zoom).max(100.0);
                let zoomed_h = (h as f32 * pf.zoom).max(100.0);

                content = content.push(
                    scrollable(
                        container(
                            image(handle)
                                .width(Length::Fixed(zoomed_w))
                                .height(Length::Fixed(zoomed_h)),
                        )
                        .center_x(Length::Fill)
                        .padding(8),
                    )
                    .height(Length::Fill),
                );
            } else {
                content = content.push(
                    text("PDF rendering failed. Is poppler-utils (pdftoppm) installed?").size(14),
                );
            }
        }
        FileCategory::Document | FileCategory::Code => {
            // Text/code preview with syntax highlighting for code
            let text_content = match std::fs::read_to_string(file_path) {
                Ok(s) => {
                    if s.len() > 50_000 {
                        format!("{}...\n\n[Preview truncated at 50KB]", &s[..50_000])
                    } else {
                        s
                    }
                }
                Err(e) => format!("Cannot preview: {}", e),
            };

            if pf.category == FileCategory::Code {
                content = content.push(
                    scrollable(render_syntax_highlighted(&text_content, file_path))
                        .height(Length::Fill),
                );
            } else {
                content = content.push(
                    scrollable(
                        container(text(text_content).size(13).font(iced::Font::MONOSPACE))
                            .padding(12)
                            .width(Length::Fill)
                            .style(container::bordered_box),
                    )
                    .height(Length::Fill),
                );
            }
        }
        FileCategory::Archive => {
            content = content.push(
                column![
                    text("Archive contents preview not available").size(14),
                    button(text("Open with system").size(14))
                        .on_press(FinderMessage::OpenWithSystem(pf.path.clone())),
                ]
                .spacing(10),
            );
        }
    }

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(4)
        .into()
}

// ── Helpers ─────────────────────────────────────────────────

fn get_image_dimensions(path: &Path) -> Option<(u32, u32)> {
    // Use `identify` or parse header manually - avoid image crate version conflicts.
    // Try imagemagick's identify first, fall back to a reasonable default.
    let output = std::process::Command::new("identify")
        .args(["-format", "%w %h", &path.to_string_lossy()])
        .output()
        .ok()?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() >= 2 {
            let w = parts[0].parse().ok()?;
            let h = parts[1].parse().ok()?;
            return Some((w, h));
        }
    }
    // Try ffprobe for video frames/images
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
            &path.to_string_lossy(),
        ])
        .output()
        .ok()?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = s.trim().split('x').collect();
        if parts.len() >= 2 {
            let w = parts[0].parse().ok()?;
            let h = parts[1].parse().ok()?;
            return Some((w, h));
        }
    }
    None
}

/// Render code with syntax highlighting using syntect.
fn render_syntax_highlighted<'a>(code: &str, file_path: &Path) -> Element<'a, FinderMessage> {
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];

    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("txt");
    let syntax = ps
        .find_syntax_by_extension(ext)
        .unwrap_or_else(|| ps.find_syntax_plain_text());

    let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);
    let mut spans_all: Vec<iced::widget::text::Span<'a, FinderMessage>> = Vec::new();

    for line in syntect::util::LinesWithEndings::from(code) {
        match highlighter.highlight_line(line, &ps) {
            Ok(ranges) => {
                for (style, text_slice) in ranges {
                    let color = Color::from_rgba8(
                        style.foreground.r,
                        style.foreground.g,
                        style.foreground.b,
                        style.foreground.a as f32 / 255.0,
                    );
                    spans_all.push(
                        iced::widget::span(text_slice.to_string())
                            .color(color)
                            .font(iced::Font::MONOSPACE)
                            .size(13),
                    );
                }
            }
            Err(_) => {
                spans_all.push(
                    iced::widget::span(line.to_string())
                        .font(iced::Font::MONOSPACE)
                        .size(13),
                );
            }
        }
    }

    container(rich_text(spans_all))
        .padding(12)
        .width(Length::Fill)
        .style(container::bordered_box)
        .into()
}
