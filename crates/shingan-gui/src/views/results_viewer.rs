use crate::platform::{format_size, truncate_end, truncate_start};
use crate::tabs::duplicate_finder::{FinderMessage, PreviewFile, ResultGroup, ResultsState};
use iced::widget::{
    button, checkbox, column, container, horizontal_rule, image, rich_text, row, scrollable, span,
    text,
};
use iced::{Color, Element, Length};
use shingan_core::file_info::FileCategory;
use shingan_utils::cleanup::{KeepStrategy, RemoveMode};
use std::path::Path;

const THUMB_SIZE: f32 = 140.0;

/// Render the results screen (or the preview of one file).
pub fn view(results: &ResultsState, remove_mode: RemoveMode) -> Element<'_, FinderMessage> {
    if let Some(pf) = &results.preview {
        return render_preview(pf);
    }
    render_results_list(results, remove_mode)
}

fn render_results_list(
    results: &ResultsState,
    remove_mode: RemoveMode,
) -> Element<'_, FinderMessage> {
    let mut content = column![].spacing(10).padding(16);
    let (total_groups, total_files) = results.counts();

    content = content.push(
        row![
            text(format!(
                "{total_groups} duplicate groups · {total_files} files"
            ))
            .size(20),
            iced::widget::horizontal_space(),
            button(text("Export CSV").size(13))
                .style(button::secondary)
                .on_press(FinderMessage::ExportResults),
            button(text("Back").size(14)).on_press(FinderMessage::CloseResults),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    );
    content = content.push(horizontal_rule(1));

    // Category filter with counts
    let mut filter_row = row![].spacing(4);
    let count = |cat: Option<FileCategory>| {
        results
            .groups
            .iter()
            .filter(|g| cat.is_none_or(|c| g.category == c))
            .count()
    };
    for (cat, label) in [
        (None, "All"),
        (Some(FileCategory::Image), "Images"),
        (Some(FileCategory::Document), "Documents"),
        (Some(FileCategory::Video), "Videos"),
        (Some(FileCategory::Archive), "Archives"),
        (Some(FileCategory::Code), "Code"),
    ] {
        let n = count(cat);
        if cat.is_some() && n == 0 {
            continue;
        }
        let btn = button(text(format!("{label} ({n})")).size(13))
            .on_press(FinderMessage::FilterChanged(cat));
        filter_row = filter_row.push(if results.filter == cat {
            btn.style(button::primary)
        } else {
            btn.style(button::secondary)
        });
    }
    content = content.push(filter_row);

    // Quick selection
    let mut select_row = row![text("Select extra copies, keeping the:").size(13)]
        .spacing(6)
        .align_y(iced::Alignment::Center);
    for (strategy, label) in [
        (KeepStrategy::Oldest, "Oldest"),
        (KeepStrategy::Newest, "Newest"),
        (KeepStrategy::Largest, "Largest"),
        (KeepStrategy::ShortestPath, "Shortest path"),
    ] {
        select_row = select_row.push(
            button(text(label).size(13))
                .style(button::secondary)
                .on_press(FinderMessage::ApplyKeep(strategy)),
        );
    }
    select_row = select_row.push(
        button(text("Clear selection").size(13))
            .style(button::text)
            .on_press(FinderMessage::SelectNone),
    );
    content = content.push(select_row.wrap());

    if let Some(msg) = &results.message {
        content = content.push(container(text(msg.as_str()).size(13)).padding([4, 8]));
    }

    // Groups on this page
    let page_indices = results.page_indices();
    let page = results.page.min(results.last_page());
    let mut groups_content = column![].spacing(14);
    for (n, i) in page_indices.iter().enumerate() {
        groups_content = groups_content.push(render_group(
            page * results.page_size + n + 1,
            &results.groups[*i],
            results,
        ));
    }
    if page_indices.is_empty() {
        groups_content = groups_content.push(text("No groups left. Nice and tidy!").size(14));
    }
    content = content.push(scrollable(groups_content).height(Length::Fill));

    // Pagination
    let last_page = results.last_page();
    let nav = |label: &'static str, msg: FinderMessage, enabled: bool| {
        let b = button(text(label).size(13)).style(button::secondary);
        if enabled {
            b.on_press(msg)
        } else {
            b
        }
    };
    if last_page > 0 {
        content = content.push(
            row![
                nav("First", FinderMessage::PageFirst, page > 0),
                nav("Prev", FinderMessage::PagePrev, page > 0),
                text(format!("Page {} of {}", page + 1, last_page + 1)).size(13),
                nav("Next", FinderMessage::PageNext, page < last_page),
                nav("Last", FinderMessage::PageLast, page < last_page),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );
    }

    // Action bar
    content = content.push(horizontal_rule(1));
    let n_selected = results.selected_for_deletion.len();
    let verb = match remove_mode {
        RemoveMode::Trash => "Move to trash",
        RemoveMode::Permanent => "Delete permanently",
    };
    let action_bar: Element<'_, FinderMessage> = if results.confirm_delete {
        let warning = match remove_mode {
            RemoveMode::Trash => format!(
                "Move {n_selected} files ({}) to the trash? At least one copy of every group is always kept.",
                format_size(results.selected_bytes())
            ),
            RemoveMode::Permanent => format!(
                "PERMANENTLY delete {n_selected} files ({})? This cannot be undone. At least one copy of every group is always kept.",
                format_size(results.selected_bytes())
            ),
        };
        row![
            text(warning).size(14).width(Length::Fill),
            button(text("Cancel").size(14))
                .style(button::secondary)
                .on_press(FinderMessage::CancelDelete),
            button(text(format!("Yes, {}", verb.to_lowercase())).size(14))
                .style(button::danger)
                .on_press(FinderMessage::ConfirmDelete),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
    } else {
        let mut del = button(text(format!("{verb}…")).size(14)).style(button::danger);
        if n_selected > 0 && !results.deleting {
            del = del.on_press(FinderMessage::DeleteSelected);
        }
        row![
            text(if n_selected == 0 {
                "Tick files to remove, or use a \"keep\" button above.".to_string()
            } else {
                format!("{n_selected} files selected")
            })
            .size(14),
            iced::widget::horizontal_space(),
            del,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
    };
    content = content.push(action_bar);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(4)
        .into()
}

fn render_group<'a>(
    index: usize,
    g: &'a ResultGroup,
    results: &'a ResultsState,
) -> Element<'a, FinderMessage> {
    let similarity = if g.group.similarity >= 0.9995 {
        "identical".to_string()
    } else {
        format!("{:.1}% similar", g.group.similarity * 100.0)
    };
    let mut files_row = row![].spacing(12);
    for file in &g.group.files {
        files_row = files_row.push(render_file_card(file, g.category, results));
    }

    container(
        column![
            text(format!(
                "#{index} · {} · {similarity} · {} files",
                g.category.label(),
                g.group.files.len()
            ))
            .size(14),
            scrollable(files_row).direction(scrollable::Direction::Horizontal(
                scrollable::Scrollbar::default(),
            )),
        ]
        .spacing(8),
    )
    .padding(12)
    .width(Length::Fill)
    .style(container::bordered_box)
    .into()
}

fn file_type_icon(category: FileCategory, ext: &str) -> (&'static str, Color) {
    match category {
        FileCategory::Image => ("IMG", Color::from_rgb(0.3, 0.7, 0.9)),
        FileCategory::Video => ("VID", Color::from_rgb(0.9, 0.4, 0.4)),
        FileCategory::Document => match ext {
            "pdf" => ("PDF", Color::from_rgb(0.9, 0.2, 0.2)),
            "doc" | "docx" => ("DOC", Color::from_rgb(0.2, 0.4, 0.9)),
            "odt" => ("ODT", Color::from_rgb(0.2, 0.6, 0.9)),
            "txt" => ("TXT", Color::from_rgb(0.6, 0.6, 0.6)),
            "rtf" => ("RTF", Color::from_rgb(0.5, 0.5, 0.8)),
            _ => ("DOC", Color::from_rgb(0.5, 0.5, 0.7)),
        },
        FileCategory::Code => match ext {
            "rs" => ("RS", Color::from_rgb(0.9, 0.5, 0.2)),
            "py" => ("PY", Color::from_rgb(0.3, 0.6, 0.9)),
            "js" | "jsx" => ("JS", Color::from_rgb(0.85, 0.75, 0.2)),
            "ts" | "tsx" => ("TS", Color::from_rgb(0.2, 0.5, 0.9)),
            "go" => ("GO", Color::from_rgb(0.0, 0.7, 0.8)),
            "c" | "cpp" | "h" => ("C", Color::from_rgb(0.4, 0.5, 0.9)),
            "html" => ("HTM", Color::from_rgb(0.9, 0.4, 0.2)),
            "css" => ("CSS", Color::from_rgb(0.3, 0.5, 0.9)),
            "vue" => ("VUE", Color::from_rgb(0.3, 0.8, 0.4)),
            "exs" => ("EX", Color::from_rgb(0.5, 0.3, 0.7)),
            _ => ("CODE", Color::from_rgb(0.6, 0.8, 0.3)),
        },
        FileCategory::Archive => match ext {
            "zip" => ("ZIP", Color::from_rgb(0.9, 0.7, 0.2)),
            "7z" => ("7Z", Color::from_rgb(0.5, 0.7, 0.3)),
            "rar" => ("RAR", Color::from_rgb(0.6, 0.3, 0.7)),
            _ => ("ARC", Color::from_rgb(0.7, 0.5, 0.2)),
        },
    }
}

fn render_file_card<'a>(
    file_path: &'a Path,
    category: FileCategory,
    results: &'a ResultsState,
) -> Element<'a, FinderMessage> {
    let path_str = file_path.to_string_lossy().to_string();
    let is_selected = results.selected_for_deletion.contains(file_path);
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
        .map(|p| truncate_start(&p.to_string_lossy(), 28))
        .unwrap_or_default();
    let meta = std::fs::metadata(file_path).ok();
    let size = meta
        .as_ref()
        .map(|m| format_size(m.len()))
        .unwrap_or_else(|| "missing".to_string());

    let thumb: Element<'a, FinderMessage> = match results.thumbnails.get(file_path) {
        Some(t) => image(image::Handle::from_path(t))
            .width(Length::Fixed(THUMB_SIZE))
            .height(Length::Fixed(THUMB_SIZE))
            .into(),
        None => {
            let (icon, color) = file_type_icon(category, &ext);
            let waiting = matches!(category, FileCategory::Image | FileCategory::Video)
                && !results.thumbs_requested.contains(file_path);
            column![
                text(icon).size(22).color(color),
                text(if waiting { "…" } else { "" }).size(10),
            ]
            .spacing(4)
            .width(Length::Fixed(THUMB_SIZE))
            .height(Length::Fixed(THUMB_SIZE))
            .align_x(iced::Alignment::Center)
            .into()
        }
    };
    let thumb_button = button(container(thumb).center_y(Length::Fixed(THUMB_SIZE)))
        .on_press(FinderMessage::PreviewFile(path_str.clone(), category))
        .style(if is_selected {
            button::danger
        } else {
            button::text
        })
        .padding(2);

    let card = column![
        thumb_button,
        text(truncate_end(&file_name, 22)).size(13),
        button(text(parent_dir).size(10))
            .on_press(FinderMessage::OpenInExplorer(path_str.clone()))
            .style(button::text)
            .padding(0),
        text(size).size(11),
        checkbox("Remove", is_selected)
            .on_toggle(move |_| FinderMessage::ToggleFileForDeletion(file_path.to_path_buf()))
            .size(16)
            .text_size(12),
    ]
    .spacing(4)
    .width(Length::Fixed(THUMB_SIZE + 20.0));

    container(card).padding(8).into()
}

fn zoom_controls<'a>(zoom: f32) -> iced::widget::Row<'a, FinderMessage> {
    row![
        button(text("-").size(16)).on_press(FinderMessage::ZoomOut),
        text(format!("{:.0}%", zoom * 100.0)).size(14),
        button(text("+").size(16)).on_press(FinderMessage::ZoomIn),
        button(text("Reset").size(12))
            .style(button::secondary)
            .on_press(FinderMessage::ZoomReset),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
}

fn zoomed_image<'a>(
    path: &Path,
    dims: Option<(u32, u32)>,
    zoom: f32,
) -> Element<'a, FinderMessage> {
    let (w, h) = dims.unwrap_or((800, 600));
    scrollable(
        container(
            image(image::Handle::from_path(path))
                .width(Length::Fixed((w as f32 * zoom).max(64.0)))
                .height(Length::Fixed((h as f32 * zoom).max(64.0))),
        )
        .center_x(Length::Fill)
        .padding(8),
    )
    .direction(scrollable::Direction::Both {
        vertical: scrollable::Scrollbar::default(),
        horizontal: scrollable::Scrollbar::default(),
    })
    .height(Length::Fill)
    .into()
}

fn render_preview(pf: &PreviewFile) -> Element<'_, FinderMessage> {
    let file_path = Path::new(&pf.path);
    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| pf.path.clone());
    let size = std::fs::metadata(file_path)
        .map(|m| format_size(m.len()))
        .unwrap_or_else(|_| "missing".to_string());

    let mut content = column![].spacing(10).padding(16);
    content = content.push(
        row![
            text(format!("{} ({})", truncate_end(&file_name, 60), size)).size(18),
            iced::widget::horizontal_space(),
            button(text("Open").size(12))
                .on_press(FinderMessage::OpenWithSystem(pf.path.clone()))
                .style(button::secondary),
            button(text("Show in folder").size(12))
                .on_press(FinderMessage::OpenInExplorer(pf.path.clone()))
                .style(button::secondary),
            button(text("Back").size(14)).on_press(FinderMessage::ClosePreview),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    );
    content = content.push(text(pf.path.clone()).size(11).style(text::secondary));
    content = content.push(horizontal_rule(1));

    let is_pdf = pf.path.to_lowercase().ends_with(".pdf");
    if is_pdf {
        content = content.push(
            row![
                button(text("< Prev page").size(13)).on_press(FinderMessage::PdfPrevPage),
                text(format!("Page {}", pf.pdf_page + 1)).size(14),
                button(text("Next page >").size(13)).on_press(FinderMessage::PdfNextPage),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );
    }

    let Some(data) = &pf.data else {
        content = content.push(text("Loading preview…").size(14));
        return container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    };

    if let Some(err) = &data.error {
        content = content.push(text(err.as_str()).size(13).style(text::secondary));
    }
    if let Some(img) = &data.image {
        content = content.push(zoom_controls(pf.zoom));
        content = content.push(zoomed_image(img, data.dimensions, pf.zoom));
    } else if let Some(spans) = &data.spans {
        let rich: Vec<iced::widget::text::Span<'_, FinderMessage>> = spans
            .iter()
            .map(|(s, c)| {
                let sp = span(s.as_str()).font(iced::Font::MONOSPACE).size(13);
                match c {
                    Some([r, g, b, a]) => {
                        sp.color(Color::from_rgba8(*r, *g, *b, *a as f32 / 255.0))
                    }
                    None => sp,
                }
            })
            .collect();
        let mut body = column![rich_text(rich)];
        if data.truncated {
            body = body.push(text("[Preview truncated]").size(12).style(text::secondary));
        }
        content = content.push(
            scrollable(
                container(body)
                    .padding(12)
                    .width(Length::Fill)
                    .style(container::bordered_box),
            )
            .height(Length::Fill),
        );
    } else if data.error.is_none() {
        content = content.push(text("Nothing to preview.").size(14));
    }
    if pf.category == FileCategory::Video {
        content = content.push(
            button(text("Play in video player").size(13))
                .on_press(FinderMessage::OpenWithSystem(pf.path.clone())),
        );
    }

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(4)
        .into()
}
