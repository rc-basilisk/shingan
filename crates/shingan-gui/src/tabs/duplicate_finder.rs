use shingan_core::file_info::FileCategory;
use shingan_core::scanner::duplicate::{ScanControl, ScanProgress};
use shingan_core::scanner::grouping::DuplicateGroup;
use iced::widget::{button, checkbox, column, container, progress_bar, row, scrollable, slider, text, Rule};
use iced::{Element, Length, Task};
use std::collections::HashMap;
use std::sync::Arc;

/// State for the Duplicate Finder tab.
pub struct DuplicateFinderState {
    pub paths: Vec<String>,
    pub include_subdirs: bool,
    pub file_types: FileTypeSelection,
    pub threshold: u8,
    pub scan_state: ScanState,
    pub current_session_id: Option<i64>,
    pub results: Option<ResultsState>,
    pub scan_control: Option<Arc<ScanControl>>,
    pub status_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileTypeSelection {
    pub image: bool,
    pub document: bool,
    pub video: bool,
    pub archive: bool,
    pub code: bool,
}

impl Default for FileTypeSelection {
    fn default() -> Self {
        Self {
            image: true,
            document: true,
            video: false,
            archive: false,
            code: false,
        }
    }
}

impl FileTypeSelection {
    pub fn selected_categories(&self) -> Vec<FileCategory> {
        let mut cats = Vec::new();
        if self.image {
            cats.push(FileCategory::Image);
        }
        if self.document {
            cats.push(FileCategory::Document);
        }
        if self.video {
            cats.push(FileCategory::Video);
        }
        if self.archive {
            cats.push(FileCategory::Archive);
        }
        if self.code {
            cats.push(FileCategory::Code);
        }
        cats
    }
}

#[derive(Debug, Clone)]
pub enum ScanState {
    Idle,
    Running {
        progress: f32,
        status: String,
        elapsed_secs: f64,
        eta_secs: Option<f64>,
    },
    Paused {
        progress: f32,
        status: String,
        elapsed_secs: f64,
        eta_secs: Option<f64>,
    },
    Completed,
}

#[derive(Debug, Clone)]
pub struct ResultsState {
    pub groups: HashMap<FileCategory, Vec<DuplicateGroup>>,
    pub selected_for_deletion: std::collections::HashSet<String>,
    pub filter: Option<FileCategory>,
    pub show_results: bool,
    pub deletion_message: Option<String>,
    pub preview: Option<PreviewFile>,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Debug, Clone)]
pub struct PreviewFile {
    pub path: String,
    pub category: FileCategory,
    pub zoom: f32,
    /// For PDFs: which page is rendered (0-indexed).
    pub pdf_page: usize,
    /// Path to a rendered temp image (for PDF pages, video frames).
    pub rendered_image: Option<String>,
}

#[derive(Debug, Clone)]
pub enum FinderMessage {
    AddFolder,
    FolderSelected(Option<String>),
    RemovePath(usize),
    ToggleSubdirs(bool),
    ToggleFileType(FileCategory, bool),
    ThresholdChanged(u8),
    StartScan,
    PauseScan,
    ResumeScan,
    StopScan,
    ScanProgress(ScanProgress),
    ViewResults,
    CloseResults,
    FilterChanged(Option<FileCategory>),
    ToggleFileForDeletion(String),
    SelectAll,
    SelectNone,
    KeepNewest,
    KeepLargest,
    DeleteSelected,
    ExportResults,
    PreviewFile(String, FileCategory),
    ClosePreview,
    OpenInExplorer(String),
    OpenWithSystem(String),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    PdfNextPage,
    PdfPrevPage,
    PageNext,
    PagePrev,
    PageFirst,
    PageLast,
}

impl Default for DuplicateFinderState {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            include_subdirs: true,
            file_types: FileTypeSelection::default(),
            threshold: 95,
            scan_state: ScanState::Idle,
            current_session_id: None,
            results: None,
            scan_control: None,
            status_message: None,
        }
    }
}

impl DuplicateFinderState {
    pub fn update(&mut self, message: FinderMessage) -> Task<FinderMessage> {
        match message {
            FinderMessage::AddFolder => {
                // Use rfd for folder selection (blocking for now)
                return Task::perform(
                    async {
                        let handle = rfd::AsyncFileDialog::new().pick_folder().await;
                        handle.map(|h| h.path().to_string_lossy().to_string())
                    },
                    FinderMessage::FolderSelected,
                );
            }
            FinderMessage::FolderSelected(Some(path)) => {
                if !self.paths.contains(&path) {
                    self.paths.push(path);
                }
            }
            FinderMessage::FolderSelected(None) => {}
            FinderMessage::RemovePath(idx) => {
                if idx < self.paths.len() {
                    self.paths.remove(idx);
                }
            }
            FinderMessage::ToggleSubdirs(val) => {
                self.include_subdirs = val;
            }
            FinderMessage::ToggleFileType(cat, enabled) => match cat {
                FileCategory::Image => self.file_types.image = enabled,
                FileCategory::Document => self.file_types.document = enabled,
                FileCategory::Video => self.file_types.video = enabled,
                FileCategory::Archive => self.file_types.archive = enabled,
                FileCategory::Code => self.file_types.code = enabled,
            },
            FinderMessage::ThresholdChanged(val) => {
                self.threshold = val.clamp(80, 100);
            }
            FinderMessage::StartScan => {
                self.scan_state = ScanState::Running {
                    progress: 0.0,
                    status: "Starting scan...".to_string(),
                    elapsed_secs: 0.0,
                    eta_secs: None,
                };
                self.results = None;
                self.status_message = None;
                let control = Arc::new(ScanControl::new());
                self.scan_control = Some(control);
            }
            FinderMessage::PauseScan => {
                if let Some(control) = &self.scan_control {
                    control.pause();
                }
                if let ScanState::Running {
                    progress,
                    elapsed_secs,
                    ..
                } = &self.scan_state
                {
                    self.scan_state = ScanState::Paused {
                        progress: *progress,
                        status: "Paused".to_string(),
                        elapsed_secs: *elapsed_secs,
                        eta_secs: None,
                    };
                }
            }
            FinderMessage::ResumeScan => {
                if let Some(control) = &self.scan_control {
                    control.resume();
                }
                if let ScanState::Paused {
                    progress,
                    elapsed_secs,
                    ..
                } = &self.scan_state
                {
                    self.scan_state = ScanState::Running {
                        progress: *progress,
                        status: "Resuming...".to_string(),
                        elapsed_secs: *elapsed_secs,
                        eta_secs: None,
                    };
                }
            }
            FinderMessage::StopScan => {
                if let Some(control) = &self.scan_control {
                    control.stop();
                }
                self.scan_state = ScanState::Idle;
            }
            FinderMessage::ScanProgress(progress) => match progress {
                ScanProgress::Status(s) => {
                    if let ScanState::Running { ref mut status, .. } = self.scan_state {
                        *status = s;
                    }
                }
                ScanProgress::Progress {
                    current,
                    total,
                    message,
                    elapsed_secs: msg_elapsed,
                    eta_secs: msg_eta,
                } => {
                    if let ScanState::Running {
                        ref mut progress,
                        ref mut status,
                        ref mut elapsed_secs,
                        ref mut eta_secs,
                    } = self.scan_state
                    {
                        *progress = if total > 0 {
                            current as f32 / total as f32
                        } else {
                            0.0
                        };
                        *status = message;
                        *elapsed_secs = msg_elapsed;
                        *eta_secs = msg_eta;
                    }
                }
                ScanProgress::PhaseCompleted { category, groups } => {
                    let results = self.results.get_or_insert(ResultsState {
                        groups: HashMap::new(),
                        selected_for_deletion: Default::default(),
                        filter: None,
                        show_results: false,
                        deletion_message: None,
                        preview: None,
                        page: 0,
                        page_size: 50,
                    });
                    results.groups.insert(category, groups);
                }
                ScanProgress::Completed => {
                    self.scan_state = ScanState::Completed;
                    self.scan_control = None;

                    // Auto-show results if duplicates were found, otherwise notify
                    if let Some(ref mut results) = self.results {
                        let total_groups: usize =
                            results.groups.values().map(|g| g.len()).sum();
                        let total_files: usize = results
                            .groups
                            .values()
                            .flat_map(|g| g.iter())
                            .map(|g| g.files.len())
                            .sum();
                        if total_groups > 0 {
                            results.show_results = true;
                            self.status_message = Some(format!(
                                "Scan complete! Found {} duplicate groups ({} files)",
                                total_groups, total_files
                            ));
                        } else {
                            self.status_message =
                                Some("Scan complete. No duplicates found.".to_string());
                        }
                    } else {
                        self.status_message =
                            Some("Scan complete. No duplicates found.".to_string());
                    }
                }
                ScanProgress::Error(e) => {
                    self.scan_state = ScanState::Idle;
                    self.scan_control = None;
                    eprintln!("Scan error: {}", e);
                }
            },
            FinderMessage::ViewResults => {
                if let Some(ref mut results) = self.results {
                    results.show_results = true;
                }
            }
            FinderMessage::CloseResults => {
                if let Some(ref mut results) = self.results {
                    results.show_results = false;
                }
            }
            FinderMessage::FilterChanged(filter) => {
                if let Some(ref mut results) = self.results {
                    results.filter = filter;
                    results.page = 0;
                }
            }
            FinderMessage::ToggleFileForDeletion(path) => {
                if let Some(ref mut results) = self.results {
                    if results.selected_for_deletion.contains(&path) {
                        results.selected_for_deletion.remove(&path);
                    } else {
                        results.selected_for_deletion.insert(path);
                    }
                }
            }
            FinderMessage::SelectAll => {
                if let Some(ref mut results) = self.results {
                    // Select all files across all visible groups
                    for (category, groups) in &results.groups {
                        if let Some(f) = &results.filter {
                            if category != f {
                                continue;
                            }
                        }
                        for group in groups {
                            for file in &group.files {
                                results
                                    .selected_for_deletion
                                    .insert(file.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
            FinderMessage::SelectNone => {
                if let Some(ref mut results) = self.results {
                    results.selected_for_deletion.clear();
                }
            }
            FinderMessage::KeepNewest => {
                if let Some(ref mut results) = self.results {
                    results.selected_for_deletion.clear();
                    for (category, groups) in &results.groups {
                        if let Some(f) = &results.filter {
                            if category != f {
                                continue;
                            }
                        }
                        for group in groups {
                            if group.files.len() < 2 {
                                continue;
                            }
                            // Find newest file by modification time
                            let newest = group
                                .files
                                .iter()
                                .max_by_key(|f| {
                                    std::fs::metadata(f)
                                        .and_then(|m| m.modified())
                                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                                });
                            // Select all except newest for deletion
                            for file in &group.files {
                                if newest.map(|n| n != file).unwrap_or(false) {
                                    results
                                        .selected_for_deletion
                                        .insert(file.to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
            }
            FinderMessage::KeepLargest => {
                if let Some(ref mut results) = self.results {
                    results.selected_for_deletion.clear();
                    for (category, groups) in &results.groups {
                        if let Some(f) = &results.filter {
                            if category != f {
                                continue;
                            }
                        }
                        for group in groups {
                            if group.files.len() < 2 {
                                continue;
                            }
                            // Find largest file by size
                            let largest = group
                                .files
                                .iter()
                                .max_by_key(|f| {
                                    std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)
                                });
                            // Select all except largest for deletion
                            for file in &group.files {
                                if largest.map(|l| l != file).unwrap_or(false) {
                                    results
                                        .selected_for_deletion
                                        .insert(file.to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
            }
            FinderMessage::DeleteSelected => {
                if let Some(ref mut results) = self.results {
                    let to_delete: Vec<String> =
                        results.selected_for_deletion.iter().cloned().collect();
                    let mut deleted = 0u32;
                    let mut failed = Vec::new();

                    for path_str in &to_delete {
                        match std::fs::remove_file(path_str) {
                            Ok(_) => {
                                deleted += 1;
                                // Remove from groups
                                for groups in results.groups.values_mut() {
                                    for group in groups.iter_mut() {
                                        group.files.retain(|f| {
                                            f.to_string_lossy() != path_str.as_str()
                                        });
                                    }
                                }
                            }
                            Err(e) => {
                                failed.push(format!("{}: {}", path_str, e));
                            }
                        }
                    }

                    results.selected_for_deletion.clear();

                    // Remove empty groups
                    for groups in results.groups.values_mut() {
                        groups.retain(|g| g.files.len() >= 2);
                    }

                    results.deletion_message = Some(format!(
                        "Deleted {} files{}",
                        deleted,
                        if failed.is_empty() {
                            String::new()
                        } else {
                            format!(", {} failed", failed.len())
                        }
                    ));
                }
            }
            FinderMessage::ExportResults => {
                if let Some(ref results) = self.results {
                    // Build CSV content
                    let mut csv =
                        String::from("group_id,file_type,similarity,file_path,file_size_bytes\n");
                    let mut group_id = 0u32;
                    for (category, groups) in &results.groups {
                        for group in groups {
                            group_id += 1;
                            for file in &group.files {
                                let size = std::fs::metadata(file)
                                    .map(|m| m.len())
                                    .unwrap_or(0);
                                csv.push_str(&format!(
                                    "{},{},{:.4},\"{}\",{}\n",
                                    group_id,
                                    category.label(),
                                    group.similarity,
                                    file.to_string_lossy().replace('"', "\"\""),
                                    size,
                                ));
                            }
                        }
                    }

                    let csv_clone = csv;
                    return Task::perform(
                        async move {
                            let handle = rfd::AsyncFileDialog::new()
                                .set_file_name("duplicates.csv")
                                .add_filter("CSV", &["csv"])
                                .add_filter("Text", &["txt"])
                                .save_file()
                                .await;
                            if let Some(h) = handle {
                                let _ = std::fs::write(h.path(), csv_clone);
                            }
                        },
                        |_| FinderMessage::SelectNone, // no-op after export
                    );
                }
            }
            FinderMessage::PreviewFile(path, category) => {
                if let Some(ref mut results) = self.results {
                    // Pre-render for PDFs and videos
                    let rendered = render_preview_image(&path, &category, 0);
                    results.preview = Some(PreviewFile {
                        path,
                        category,
                        zoom: 1.0,
                        pdf_page: 0,
                        rendered_image: rendered,
                    });
                }
            }
            FinderMessage::ClosePreview => {
                if let Some(ref mut results) = self.results {
                    results.preview = None;
                }
            }
            FinderMessage::OpenInExplorer(path) => {
                let file_path = std::path::Path::new(&path);
                if let Some(parent) = file_path.parent() {
                    let _ = std::process::Command::new("xdg-open")
                        .arg(parent)
                        .spawn();
                }
            }
            FinderMessage::OpenWithSystem(path) => {
                let _ = std::process::Command::new("xdg-open")
                    .arg(&path)
                    .spawn();
            }
            FinderMessage::ZoomIn => {
                if let Some(ref mut results) = self.results {
                    if let Some(ref mut pf) = results.preview {
                        pf.zoom = (pf.zoom * 1.25).min(5.0);
                    }
                }
            }
            FinderMessage::ZoomOut => {
                if let Some(ref mut results) = self.results {
                    if let Some(ref mut pf) = results.preview {
                        pf.zoom = (pf.zoom / 1.25).max(0.2);
                    }
                }
            }
            FinderMessage::ZoomReset => {
                if let Some(ref mut results) = self.results {
                    if let Some(ref mut pf) = results.preview {
                        pf.zoom = 1.0;
                    }
                }
            }
            FinderMessage::PdfNextPage => {
                if let Some(ref mut results) = self.results {
                    if let Some(ref mut pf) = results.preview {
                        pf.pdf_page += 1;
                        pf.rendered_image =
                            render_preview_image(&pf.path, &pf.category, pf.pdf_page);
                    }
                }
            }
            FinderMessage::PdfPrevPage => {
                if let Some(ref mut results) = self.results {
                    if let Some(ref mut pf) = results.preview {
                        if pf.pdf_page > 0 {
                            pf.pdf_page -= 1;
                            pf.rendered_image =
                                render_preview_image(&pf.path, &pf.category, pf.pdf_page);
                        }
                    }
                }
            }
            FinderMessage::PageNext => {
                if let Some(ref mut results) = self.results {
                    let total: usize = results
                        .groups
                        .iter()
                        .filter(|(c, _)| results.filter.is_none_or(|f| *c == &f))
                        .map(|(_, g)| g.len())
                        .sum();
                    let last_page = if results.page_size > 0 {
                        total.saturating_sub(1) / results.page_size
                    } else {
                        0
                    };
                    if results.page < last_page {
                        results.page += 1;
                    }
                }
            }
            FinderMessage::PagePrev => {
                if let Some(ref mut results) = self.results {
                    results.page = results.page.saturating_sub(1);
                }
            }
            FinderMessage::PageFirst => {
                if let Some(ref mut results) = self.results {
                    results.page = 0;
                }
            }
            FinderMessage::PageLast => {
                if let Some(ref mut results) = self.results {
                    let total: usize = results
                        .groups
                        .iter()
                        .filter(|(c, _)| results.filter.is_none_or(|f| *c == &f))
                        .map(|(_, g)| g.len())
                        .sum();
                    results.page = if results.page_size > 0 {
                        total.saturating_sub(1) / results.page_size
                    } else {
                        0
                    };
                }
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, FinderMessage> {
        let mut content = column![].spacing(12).padding([16, 24]);

        // -- Scan Paths --
        content = content.push(text("Scan Paths").size(16));

        for (i, path) in self.paths.iter().enumerate() {
            content = content.push(
                container(
                    row![
                        text(path).size(13).width(Length::Fill),
                        button(text("Remove").size(12))
                            .padding([4, 10])
                            .on_press(FinderMessage::RemovePath(i)),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                )
                .padding([6, 12]),
            );
        }

        content = content.push(
            row![
                button("Add Folder").on_press(FinderMessage::AddFolder),
                checkbox("Include subdirectories", self.include_subdirs)
                    .on_toggle(FinderMessage::ToggleSubdirs),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
        );

        // -- File Types --
        content = content.push(Rule::horizontal(1));
        content = content.push(text("File Types").size(16));
        content = content.push(
            row![
                checkbox("Images", self.file_types.image)
                    .on_toggle(|v| FinderMessage::ToggleFileType(FileCategory::Image, v)),
                checkbox("Documents", self.file_types.document)
                    .on_toggle(|v| FinderMessage::ToggleFileType(FileCategory::Document, v)),
                checkbox("Videos", self.file_types.video)
                    .on_toggle(|v| FinderMessage::ToggleFileType(FileCategory::Video, v)),
                checkbox("Archives", self.file_types.archive)
                    .on_toggle(|v| FinderMessage::ToggleFileType(FileCategory::Archive, v)),
                checkbox("Code", self.file_types.code)
                    .on_toggle(|v| FinderMessage::ToggleFileType(FileCategory::Code, v)),
            ]
            .spacing(15),
        );

        // -- Threshold --
        content = content.push(Rule::horizontal(1));
        content = content.push(
            row![
                text(format!("Similarity Threshold: {}%", self.threshold)),
                slider(80..=100, self.threshold, FinderMessage::ThresholdChanged).width(200),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
        );

        // -- Controls --
        content = content.push(Rule::horizontal(1));
        let controls = match &self.scan_state {
            ScanState::Idle | ScanState::Completed => {
                let mut start = button(text("Start Scan").size(14)).padding([8, 24]);
                if !self.paths.is_empty() {
                    start = start.on_press(FinderMessage::StartScan);
                }
                let mut r = row![start].spacing(10);
                if matches!(self.scan_state, ScanState::Completed)
                    && self.results.is_some()
                {
                    r = r.push(
                        button(text("View Results").size(14))
                            .padding([8, 24])
                            .on_press(FinderMessage::ViewResults),
                    );
                }
                r
            }
            ScanState::Running { .. } => {
                row![
                    button(text("Pause").size(13)).padding([6, 16]).on_press(FinderMessage::PauseScan),
                    button(text("Stop").size(13)).padding([6, 16]).on_press(FinderMessage::StopScan),
                ]
                .spacing(10)
            }
            ScanState::Paused { .. } => {
                row![
                    button(text("Resume").size(13)).padding([6, 16]).on_press(FinderMessage::ResumeScan),
                    button(text("Stop").size(13)).padding([6, 16]).on_press(FinderMessage::StopScan),
                ]
                .spacing(10)
            }
        };
        content = content.push(controls);

        // -- Progress --
        match &self.scan_state {
            ScanState::Running {
                progress,
                status,
                elapsed_secs,
                eta_secs,
            }
            | ScanState::Paused {
                progress,
                status,
                elapsed_secs,
                eta_secs,
            } => {
                let progress_section = {
                    let mut timing = format!("{} | Elapsed: {}", status, format_duration(*elapsed_secs));
                    if let Some(eta) = eta_secs {
                        timing.push_str(&format!(" | ETA: {}", format_duration(*eta)));
                    }
                    container(
                        column![
                            progress_bar(0.0..=1.0, *progress).height(20),
                            text(timing).size(13),
                        ]
                        .spacing(6),
                    )
                    .padding([10, 0])
                };
                content = content.push(progress_section);
            }
            _ => {}
        }

        // -- Status message --
        if let Some(ref msg) = self.status_message {
            content = content.push(
                container(text(msg).size(14)).padding([8, 12]),
            );
        }

        scrollable(container(content).width(Length::Fill)).into()
    }
}

/// Render a preview image for PDFs (via pdftoppm) or videos (via ffmpeg).
/// Returns the path to a temp PNG file, or None if rendering fails.
fn format_duration(secs: f64) -> String {
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

fn render_preview_image(
    file_path: &str,
    category: &FileCategory,
    page: usize,
) -> Option<String> {
    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("shingan")
        .join("previews");
    std::fs::create_dir_all(&cache_dir).ok()?;

    match category {
        FileCategory::Document if file_path.ends_with(".pdf") => {
            // Use pdftoppm to render a specific page
            let out_prefix = cache_dir.join(format!(
                "pdf_{:x}_p{}",
                simple_hash(file_path),
                page
            ));
            let out_file = format!("{}-{:06}.png", out_prefix.display(), page + 1);

            // Check cache
            if std::path::Path::new(&out_file).exists() {
                return Some(out_file);
            }

            let first = (page + 1).to_string();
            let last = first.clone();
            let status = std::process::Command::new("pdftoppm")
                .args([
                    "-png",
                    "-r", "150",
                    "-f", &first,
                    "-l", &last,
                    file_path,
                    &out_prefix.to_string_lossy(),
                ])
                .output();

            match status {
                Ok(output) if output.status.success() => {
                    // pdftoppm names files as prefix-NNNNNN.png
                    if std::path::Path::new(&out_file).exists() {
                        Some(out_file)
                    } else {
                        // Try alternate naming (single digit)
                        let alt = format!("{}-{}.png", out_prefix.display(), page + 1);
                        if std::path::Path::new(&alt).exists() {
                            Some(alt)
                        } else {
                            None
                        }
                    }
                }
                _ => None,
            }
        }
        FileCategory::Video => {
            // Use ffmpeg to extract a single frame at 5 seconds
            let out_file = cache_dir.join(format!("vid_{:x}.jpg", simple_hash(file_path)));

            if out_file.exists() {
                return Some(out_file.to_string_lossy().to_string());
            }

            let status = std::process::Command::new("ffmpeg")
                .args([
                    "-ss", "5",
                    "-i", file_path,
                    "-vframes", "1",
                    "-q:v", "2",
                    "-y",
                    &out_file.to_string_lossy(),
                ])
                .stderr(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .output();

            match status {
                Ok(output) if output.status.success() && out_file.exists() => {
                    Some(out_file.to_string_lossy().to_string())
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Generate a video thumbnail for display in file cards.
/// Cached in ~/.cache/shingan/thumbnails/.
pub fn video_thumbnail_path(video_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("shingan")
        .join("thumbnails");
    std::fs::create_dir_all(&cache_dir).ok()?;

    let out_file = cache_dir.join(format!(
        "vid_{:x}.jpg",
        simple_hash(&video_path.to_string_lossy())
    ));

    if out_file.exists() {
        return Some(out_file);
    }

    let status = std::process::Command::new("ffmpeg")
        .args([
            "-ss", "3",
            "-i", &video_path.to_string_lossy(),
            "-vframes", "1",
            "-vf", "scale=280:-1",
            "-q:v", "4",
            "-y",
            &out_file.to_string_lossy(),
        ])
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .output();

    match status {
        Ok(output) if output.status.success() && out_file.exists() => Some(out_file),
        _ => None,
    }
}

fn simple_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}
