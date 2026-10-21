use crate::media::{self, PreviewData};
use crate::platform::{format_size, truncate_end};
use iced::widget::{
    button, checkbox, column, container, progress_bar, row, scrollable, slider, text, Rule,
};
use iced::{Element, Length, Task};
use shingan_core::file_info::FileCategory;
use shingan_core::scanner::duplicate::{ScanControl, ScanProgress};
use shingan_core::scanner::grouping::DuplicateGroup;
use shingan_utils::cleanup::{self, KeepStrategy, RemoveMode, RemoveReport};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// A past scan shown in the "Recent scans" list.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub status: String,
    pub groups: u64,
    pub files: u64,
}

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
    /// Bumped for every new scan so its subscription gets a fresh identity.
    pub scan_generation: u64,
    pub recent_sessions: Vec<SessionSummary>,
    /// Set by the app from settings.
    pub dark: bool,
    pub remove_mode: RemoveMode,
    /// Requests the app must carry out (database access).
    pub requests: Vec<FinderRequest>,
}

/// Work the finder asks the app to do, since the app owns the database.
#[derive(Debug, Clone)]
pub enum FinderRequest {
    OpenSession(i64),
    DeleteSession(i64),
    /// Files were removed from disk; drop them from the stored session.
    FilesRemoved {
        session_id: i64,
        paths: Vec<String>,
    },
    RefreshSessions,
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
    },
    Completed,
}

impl ScanState {
    /// Running or paused: the scanner thread is alive.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Running { .. } | Self::Paused { .. })
    }
}

/// One duplicate group as shown in the results list.
#[derive(Debug, Clone)]
pub struct ResultGroup {
    pub category: FileCategory,
    pub group: DuplicateGroup,
}

#[derive(Debug, Clone)]
pub struct ResultsState {
    pub session_id: Option<i64>,
    pub groups: Vec<ResultGroup>,
    pub selected_for_deletion: HashSet<PathBuf>,
    pub filter: Option<FileCategory>,
    pub show_results: bool,
    pub message: Option<String>,
    pub confirm_delete: bool,
    pub deleting: bool,
    pub preview: Option<PreviewFile>,
    pub page: usize,
    pub page_size: usize,
    /// file -> cached thumbnail
    pub thumbnails: HashMap<PathBuf, PathBuf>,
    /// Files whose thumbnail was already requested (successful or not).
    pub thumbs_requested: HashSet<PathBuf>,
}

impl ResultsState {
    pub fn new(session_id: Option<i64>, groups: Vec<ResultGroup>) -> Self {
        Self {
            session_id,
            groups,
            selected_for_deletion: HashSet::new(),
            filter: None,
            show_results: false,
            message: None,
            confirm_delete: false,
            deleting: false,
            preview: None,
            page: 0,
            page_size: 30,
            thumbnails: HashMap::new(),
            thumbs_requested: HashSet::new(),
        }
    }

    /// Indices (into `groups`) of the groups matching the current filter.
    pub fn visible(&self) -> Vec<usize> {
        self.groups
            .iter()
            .enumerate()
            .filter(|(_, g)| self.filter.is_none_or(|f| g.category == f))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn last_page(&self) -> usize {
        self.visible()
            .len()
            .saturating_sub(1)
            .checked_div(self.page_size)
            .unwrap_or(0)
    }

    pub fn page_indices(&self) -> Vec<usize> {
        let page = self.page.min(self.last_page());
        self.visible()
            .into_iter()
            .skip(page * self.page_size)
            .take(self.page_size)
            .collect()
    }

    /// Total size of the selected files that still exist.
    pub fn selected_bytes(&self) -> u64 {
        self.selected_for_deletion
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum()
    }

    pub fn counts(&self) -> (usize, usize) {
        let groups = self.groups.len();
        let files = self.groups.iter().map(|g| g.group.files.len()).sum();
        (groups, files)
    }
}

#[derive(Debug, Clone)]
pub struct PreviewFile {
    pub path: String,
    pub category: FileCategory,
    pub zoom: f32,
    /// For PDFs: which page is rendered (0-indexed).
    pub pdf_page: usize,
    pub data: Option<PreviewData>,
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
    OpenSession(i64),
    DeleteSession(i64),
    FilterChanged(Option<FileCategory>),
    ToggleFileForDeletion(PathBuf),
    SelectNone,
    ApplyKeep(KeepStrategy),
    DeleteSelected,
    ConfirmDelete,
    CancelDelete,
    DeleteFinished(Arc<RemoveReport>, usize),
    ExportResults,
    Exported(Result<Option<String>, String>),
    PreviewFile(String, FileCategory),
    PreviewReady(Box<PreviewData>),
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
    ThumbnailsReady(Vec<(PathBuf, Option<PathBuf>)>),
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
            scan_generation: 0,
            recent_sessions: Vec::new(),
            dark: true,
            remove_mode: RemoveMode::Trash,
            requests: Vec::new(),
        }
    }
}

fn preview_task(
    path: String,
    category: FileCategory,
    page: usize,
    dark: bool,
) -> Task<FinderMessage> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || media::load_preview(&path, category, page, dark))
                .await
                .unwrap_or_default()
        },
        |d| FinderMessage::PreviewReady(Box::new(d)),
    )
}

impl DuplicateFinderState {
    pub fn can_start(&self) -> bool {
        !self.paths.is_empty()
            && !self.file_types.selected_categories().is_empty()
            && !self.scan_state.is_active()
    }

    /// Install freshly loaded/scanned results and start making thumbnails.
    pub fn set_results(&mut self, results: ResultsState) -> Task<FinderMessage> {
        self.results = Some(results);
        self.thumbnail_task()
    }

    /// Generate thumbnails for the files on the current results page.
    fn thumbnail_task(&mut self) -> Task<FinderMessage> {
        let Some(results) = &mut self.results else {
            return Task::none();
        };
        let mut wanted: Vec<(PathBuf, FileCategory)> = Vec::new();
        for i in results.page_indices() {
            let g = &results.groups[i];
            if !matches!(g.category, FileCategory::Image | FileCategory::Video) {
                continue;
            }
            for f in &g.group.files {
                if results.thumbs_requested.insert(f.clone()) {
                    wanted.push((f.clone(), g.category));
                }
            }
        }
        if wanted.is_empty() {
            return Task::none();
        }
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    use rayon::prelude::*;
                    wanted
                        .into_par_iter()
                        .map(|(p, c)| {
                            let t = media::thumbnail(&p, c);
                            (p, t)
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default()
            },
            FinderMessage::ThumbnailsReady,
        )
    }

    pub fn update(&mut self, message: FinderMessage) -> Task<FinderMessage> {
        match message {
            FinderMessage::AddFolder => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .pick_folder()
                            .await
                            .map(|h| h.path().to_string_lossy().to_string())
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
            FinderMessage::ToggleSubdirs(val) => self.include_subdirs = val,
            FinderMessage::ToggleFileType(cat, enabled) => match cat {
                FileCategory::Image => self.file_types.image = enabled,
                FileCategory::Document => self.file_types.document = enabled,
                FileCategory::Video => self.file_types.video = enabled,
                FileCategory::Archive => self.file_types.archive = enabled,
                FileCategory::Code => self.file_types.code = enabled,
            },
            FinderMessage::ThresholdChanged(val) => self.threshold = val.clamp(70, 100),
            FinderMessage::StartScan => {
                if !self.can_start() {
                    return Task::none();
                }
                self.scan_generation += 1;
                self.scan_state = ScanState::Running {
                    progress: 0.0,
                    status: "Starting scan...".to_string(),
                    elapsed_secs: 0.0,
                    eta_secs: None,
                };
                self.results = None;
                self.status_message = None;
                self.scan_control = Some(Arc::new(ScanControl::new()));
            }
            FinderMessage::PauseScan => {
                if let Some(control) = &self.scan_control {
                    control.pause();
                }
                if let ScanState::Running {
                    progress,
                    status,
                    elapsed_secs,
                    ..
                } = &self.scan_state
                {
                    self.scan_state = ScanState::Paused {
                        progress: *progress,
                        status: status.clone(),
                        elapsed_secs: *elapsed_secs,
                    };
                }
            }
            FinderMessage::ResumeScan => {
                if let Some(control) = &self.scan_control {
                    control.resume();
                }
                if let ScanState::Paused {
                    progress,
                    status,
                    elapsed_secs,
                } = &self.scan_state
                {
                    self.scan_state = ScanState::Running {
                        progress: *progress,
                        status: status.clone(),
                        elapsed_secs: *elapsed_secs,
                        eta_secs: None,
                    };
                }
            }
            FinderMessage::StopScan => {
                if let Some(control) = &self.scan_control {
                    control.stop();
                }
                self.scan_control = None;
                self.scan_state = ScanState::Idle;
                self.status_message = Some(
                    "Scan stopped. Groups found before stopping are kept in the scan history."
                        .to_string(),
                );
                self.requests.push(FinderRequest::RefreshSessions);
            }
            FinderMessage::ScanProgress(progress) => return self.on_progress(progress),
            FinderMessage::ViewResults => {
                if let Some(ref mut results) = self.results {
                    results.show_results = true;
                }
                return self.thumbnail_task();
            }
            FinderMessage::CloseResults => {
                if let Some(ref mut results) = self.results {
                    results.show_results = false;
                    results.confirm_delete = false;
                }
            }
            FinderMessage::OpenSession(id) => self.requests.push(FinderRequest::OpenSession(id)),
            FinderMessage::DeleteSession(id) => {
                self.requests.push(FinderRequest::DeleteSession(id))
            }
            FinderMessage::FilterChanged(filter) => {
                if let Some(ref mut results) = self.results {
                    results.filter = filter;
                    results.page = 0;
                }
                return self.thumbnail_task();
            }
            FinderMessage::ToggleFileForDeletion(path) => {
                if let Some(ref mut results) = self.results {
                    if !results.selected_for_deletion.remove(&path) {
                        results.selected_for_deletion.insert(path);
                    }
                    results.confirm_delete = false;
                }
            }
            FinderMessage::SelectNone => {
                if let Some(ref mut results) = self.results {
                    results.selected_for_deletion.clear();
                    results.confirm_delete = false;
                }
            }
            FinderMessage::ApplyKeep(strategy) => {
                if let Some(ref mut results) = self.results {
                    results.selected_for_deletion.clear();
                    results.confirm_delete = false;
                    let mut groups = 0;
                    for i in results.visible() {
                        if let Some(plan) =
                            cleanup::plan_group(&results.groups[i].group.files, strategy)
                        {
                            groups += 1;
                            results.selected_for_deletion.extend(plan.remove);
                        }
                    }
                    results.message = Some(format!(
                        "{strategy}: selected {} extra copies in {groups} groups. Review, then press Remove.",
                        results.selected_for_deletion.len()
                    ));
                }
            }
            FinderMessage::DeleteSelected => {
                if let Some(ref mut results) = self.results {
                    if !results.selected_for_deletion.is_empty() {
                        results.confirm_delete = true;
                    }
                }
            }
            FinderMessage::CancelDelete => {
                if let Some(ref mut results) = self.results {
                    results.confirm_delete = false;
                }
            }
            FinderMessage::ConfirmDelete => {
                let mode = self.remove_mode;
                if let Some(ref mut results) = self.results {
                    results.confirm_delete = false;
                    let groups: Vec<&[PathBuf]> = results
                        .groups
                        .iter()
                        .map(|g| g.group.files.as_slice())
                        .collect();
                    let (safe, protected) =
                        cleanup::protect_last_copies(groups, &results.selected_for_deletion);
                    if safe.is_empty() {
                        results.message = Some(
                            "Nothing removed: every selected file is the last remaining copy in its group."
                                .to_string(),
                        );
                        return Task::none();
                    }
                    results.deleting = true;
                    results.message = Some(format!("Removing {} files...", safe.len()));
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || cleanup::remove_files(&safe, mode))
                                .await
                                .unwrap_or_default()
                        },
                        move |report| FinderMessage::DeleteFinished(Arc::new(report), protected),
                    );
                }
            }
            FinderMessage::DeleteFinished(report, protected) => {
                if let Some(ref mut results) = self.results {
                    results.deleting = false;
                    let removed: HashSet<&PathBuf> = report.removed.iter().collect();
                    for g in results.groups.iter_mut() {
                        g.group.files.retain(|f| !removed.contains(f));
                    }
                    results.groups.retain(|g| g.group.files.len() >= 2);
                    results
                        .selected_for_deletion
                        .retain(|p| !removed.contains(p));
                    results.page = results.page.min(results.last_page());

                    let verb = match self.remove_mode {
                        RemoveMode::Trash => "Moved to trash",
                        RemoveMode::Permanent => "Deleted",
                    };
                    let mut msg = format!(
                        "{verb}: {} files ({} freed).",
                        report.removed.len(),
                        format_size(report.bytes_freed)
                    );
                    if protected > 0 {
                        msg.push_str(&format!(
                            " Kept one copy in {protected} groups where every file was selected."
                        ));
                    }
                    if let Some((p, e)) = report.failed.first() {
                        msg.push_str(&format!(
                            " {} failed (e.g. {}: {e}).",
                            report.failed.len(),
                            p.display()
                        ));
                    }
                    results.message = Some(msg);

                    if let Some(session_id) = results.session_id {
                        self.requests.push(FinderRequest::FilesRemoved {
                            session_id,
                            paths: report
                                .removed
                                .iter()
                                .map(|p| p.to_string_lossy().into_owned())
                                .collect(),
                        });
                    }
                }
                return self.thumbnail_task();
            }
            FinderMessage::ExportResults => {
                if let Some(ref results) = self.results {
                    let mut csv =
                        String::from("group_id,file_type,similarity,file_path,file_size_bytes\n");
                    for (i, g) in results.groups.iter().enumerate() {
                        for file in &g.group.files {
                            let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
                            let path = file.to_string_lossy();
                            let path = if path.contains([',', '"', '\n']) {
                                format!("\"{}\"", path.replace('"', "\"\""))
                            } else {
                                path.into_owned()
                            };
                            csv.push_str(&format!(
                                "{},{},{:.4},{},{}\n",
                                i + 1,
                                g.category.label(),
                                g.group.similarity,
                                path,
                                size,
                            ));
                        }
                    }
                    return Task::perform(
                        async move {
                            let Some(h) = rfd::AsyncFileDialog::new()
                                .set_file_name("duplicates.csv")
                                .add_filter("CSV", &["csv"])
                                .save_file()
                                .await
                            else {
                                return Ok(None);
                            };
                            std::fs::write(h.path(), csv)
                                .map(|_| Some(h.path().display().to_string()))
                                .map_err(|e| e.to_string())
                        },
                        FinderMessage::Exported,
                    );
                }
            }
            FinderMessage::Exported(result) => {
                if let Some(ref mut results) = self.results {
                    results.message = match result {
                        Ok(Some(p)) => Some(format!("Exported to {p}")),
                        Ok(None) => None,
                        Err(e) => Some(format!("Export failed: {e}")),
                    };
                }
            }
            FinderMessage::PreviewFile(path, category) => {
                if let Some(ref mut results) = self.results {
                    results.preview = Some(PreviewFile {
                        path: path.clone(),
                        category,
                        zoom: 1.0,
                        pdf_page: 0,
                        data: None,
                    });
                    return preview_task(path, category, 0, self.dark);
                }
            }
            FinderMessage::PreviewReady(data) => {
                if let Some(pf) = self.results.as_mut().and_then(|r| r.preview.as_mut()) {
                    // Ignore results for a file/page that is no longer shown.
                    if pf.path == data.path && pf.pdf_page == data.page {
                        pf.data = Some(*data);
                    }
                }
            }
            FinderMessage::ClosePreview => {
                if let Some(ref mut results) = self.results {
                    results.preview = None;
                }
            }
            FinderMessage::OpenInExplorer(path) => {
                crate::platform::reveal_in_folder(std::path::Path::new(&path))
            }
            FinderMessage::OpenWithSystem(path) => {
                crate::platform::open_path(std::path::Path::new(&path))
            }
            FinderMessage::ZoomIn | FinderMessage::ZoomOut | FinderMessage::ZoomReset => {
                if let Some(pf) = self.results.as_mut().and_then(|r| r.preview.as_mut()) {
                    pf.zoom = match message {
                        FinderMessage::ZoomIn => (pf.zoom * 1.25).min(5.0),
                        FinderMessage::ZoomOut => (pf.zoom / 1.25).max(0.2),
                        _ => 1.0,
                    };
                }
            }
            FinderMessage::PdfNextPage | FinderMessage::PdfPrevPage => {
                let dark = self.dark;
                if let Some(pf) = self.results.as_mut().and_then(|r| r.preview.as_mut()) {
                    let next = matches!(message, FinderMessage::PdfNextPage);
                    if !next && pf.pdf_page == 0 {
                        return Task::none();
                    }
                    pf.pdf_page = if next {
                        pf.pdf_page + 1
                    } else {
                        pf.pdf_page - 1
                    };
                    pf.data = None;
                    return preview_task(pf.path.clone(), pf.category, pf.pdf_page, dark);
                }
            }
            FinderMessage::PageNext
            | FinderMessage::PagePrev
            | FinderMessage::PageFirst
            | FinderMessage::PageLast => {
                if let Some(ref mut results) = self.results {
                    let last = results.last_page();
                    results.page = match message {
                        FinderMessage::PageNext => (results.page + 1).min(last),
                        FinderMessage::PagePrev => results.page.saturating_sub(1),
                        FinderMessage::PageFirst => 0,
                        _ => last,
                    };
                }
                return self.thumbnail_task();
            }
            FinderMessage::ThumbnailsReady(list) => {
                if let Some(ref mut results) = self.results {
                    for (file, thumb) in list {
                        if let Some(t) = thumb {
                            results.thumbnails.insert(file, t);
                        }
                    }
                }
            }
        }
        Task::none()
    }

    fn on_progress(&mut self, progress: ScanProgress) -> Task<FinderMessage> {
        match progress {
            ScanProgress::Status(s) => match &mut self.scan_state {
                ScanState::Running { status, .. } | ScanState::Paused { status, .. } => *status = s,
                _ => {}
            },
            ScanProgress::Progress {
                current,
                total,
                message,
                elapsed_secs: msg_elapsed,
                eta_secs: msg_eta,
            } => {
                if let ScanState::Running {
                    progress,
                    status,
                    elapsed_secs,
                    eta_secs,
                } = &mut self.scan_state
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
                let results = self
                    .results
                    .get_or_insert_with(|| ResultsState::new(self.current_session_id, Vec::new()));
                results.groups.extend(
                    groups
                        .into_iter()
                        .map(|group| ResultGroup { category, group }),
                );
            }
            ScanProgress::Completed => {
                if !self.scan_state.is_active() {
                    return Task::none(); // stopped by the user
                }
                self.scan_state = ScanState::Completed;
                self.scan_control = None;
                self.requests.push(FinderRequest::RefreshSessions);
                let found = self.results.as_ref().map(|r| r.counts()).unwrap_or((0, 0));
                if found.0 > 0 {
                    self.status_message = Some(format!(
                        "Scan complete: {} duplicate groups ({} files).",
                        found.0, found.1
                    ));
                    if let Some(r) = self.results.as_mut() {
                        r.show_results = true;
                    }
                    return self.thumbnail_task();
                }
                self.results = None;
                self.status_message = Some("Scan complete. No duplicates found.".to_string());
            }
            ScanProgress::Error(e) => {
                self.scan_state = ScanState::Idle;
                self.scan_control = None;
                self.status_message = Some(format!("Scan failed: {e}"));
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, FinderMessage> {
        let active = self.scan_state.is_active();
        let mut content = column![].spacing(12).padding([16, 24]);

        // -- Scan Paths --
        content = content.push(text("Folders to scan").size(16));
        if self.paths.is_empty() {
            content = content.push(
                text("Add one or more folders, choose the kinds of files to compare, then start the scan.")
                    .size(13)
                    .style(text::secondary),
            );
        }
        for (i, path) in self.paths.iter().enumerate() {
            let mut remove = button(text("Remove").size(12))
                .padding([4, 10])
                .style(button::secondary);
            if !active {
                remove = remove.on_press(FinderMessage::RemovePath(i));
            }
            content = content.push(
                container(
                    row![text(path).size(13).width(Length::Fill), remove]
                        .spacing(10)
                        .align_y(iced::Alignment::Center),
                )
                .padding([6, 12])
                .style(container::rounded_box),
            );
        }

        let mut add = button("Add folder");
        if !active {
            add = add.on_press(FinderMessage::AddFolder);
        }
        content = content.push(
            row![
                add,
                checkbox("Include subfolders", self.include_subdirs)
                    .on_toggle_maybe((!active).then_some(FinderMessage::ToggleSubdirs)),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
        );

        // -- File Types --
        content = content.push(Rule::horizontal(1));
        content = content.push(text("Compare").size(16));
        let toggle = |label: &'static str, on: bool, cat: FileCategory| {
            checkbox(label, on).on_toggle_maybe(
                (!active).then_some(move |v| FinderMessage::ToggleFileType(cat, v)),
            )
        };
        content = content.push(
            row![
                toggle("Images", self.file_types.image, FileCategory::Image),
                toggle(
                    "Documents",
                    self.file_types.document,
                    FileCategory::Document
                ),
                toggle("Videos", self.file_types.video, FileCategory::Video),
                toggle("Archives", self.file_types.archive, FileCategory::Archive),
                toggle("Code", self.file_types.code, FileCategory::Code),
            ]
            .spacing(15),
        );

        // -- Threshold --
        let threshold_hint = match self.threshold {
            100 => "identical content only",
            96..=99 => "near-identical (resized, re-saved, lightly edited)",
            90..=95 => "similar (recommended)",
            80..=89 => "loosely similar — expect some false matches",
            _ => "very loose — many false matches",
        };
        content = content.push(
            row![
                text(format!("Similarity: {}%", self.threshold)).width(140),
                slider(70..=100, self.threshold, FinderMessage::ThresholdChanged).width(220),
                text(threshold_hint).size(12).style(text::secondary),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
        );

        // -- Controls --
        content = content.push(Rule::horizontal(1));
        let controls = match &self.scan_state {
            ScanState::Idle | ScanState::Completed => {
                let mut start = button(text("Start scan").size(14))
                    .padding([8, 24])
                    .style(button::primary);
                if self.can_start() {
                    start = start.on_press(FinderMessage::StartScan);
                }
                let mut r = row![start].spacing(10);
                if self.results.is_some() {
                    r = r.push(
                        button(text("View results").size(14))
                            .padding([8, 24])
                            .on_press(FinderMessage::ViewResults),
                    );
                }
                r
            }
            ScanState::Running { .. } => row![
                button(text("Pause").size(13))
                    .padding([6, 16])
                    .on_press(FinderMessage::PauseScan),
                button(text("Stop").size(13))
                    .padding([6, 16])
                    .style(button::danger)
                    .on_press(FinderMessage::StopScan),
            ]
            .spacing(10),
            ScanState::Paused { .. } => row![
                button(text("Resume").size(13))
                    .padding([6, 16])
                    .on_press(FinderMessage::ResumeScan),
                button(text("Stop").size(13))
                    .padding([6, 16])
                    .style(button::danger)
                    .on_press(FinderMessage::StopScan),
            ]
            .spacing(10),
        };
        content = content.push(controls);

        // -- Progress --
        match &self.scan_state {
            ScanState::Running {
                progress,
                status,
                elapsed_secs,
                eta_secs,
            } => {
                let mut timing = format!("{} · {} elapsed", status, format_duration(*elapsed_secs));
                if let Some(eta) = eta_secs {
                    timing.push_str(&format!(" · ~{} left", format_duration(*eta)));
                }
                content = content.push(
                    column![
                        progress_bar(0.0..=1.0, *progress).height(20),
                        text(timing).size(13)
                    ]
                    .spacing(6),
                );
            }
            ScanState::Paused {
                progress, status, ..
            } => {
                content = content.push(
                    column![
                        progress_bar(0.0..=1.0, *progress).height(20),
                        text(format!("Paused · {status}")).size(13)
                    ]
                    .spacing(6),
                );
            }
            _ => {}
        }

        if let Some(ref msg) = self.status_message {
            content = content.push(container(text(msg).size(14)).padding([8, 12]));
        }

        // -- Recent scans --
        if !self.recent_sessions.is_empty() {
            content = content.push(Rule::horizontal(1));
            content = content.push(text("Recent scans").size(16));
            for s in self.recent_sessions.iter().take(8) {
                let mut open = button(text("Open").size(12)).padding([4, 12]);
                if s.groups > 0 && !active {
                    open = open.on_press(FinderMessage::OpenSession(s.id));
                }
                let mut forget = button(text("Forget").size(12))
                    .padding([4, 10])
                    .style(button::secondary);
                if !active {
                    forget = forget.on_press(FinderMessage::DeleteSession(s.id));
                }
                content = content.push(
                    container(
                        row![
                            column![
                                text(truncate_end(&s.name, 60)).size(13),
                                text(format!(
                                    "{} · {} groups, {} files · {}",
                                    s.created_at, s.groups, s.files, s.status
                                ))
                                .size(11)
                                .style(text::secondary),
                            ]
                            .width(Length::Fill),
                            open,
                            forget,
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    )
                    .padding([6, 12])
                    .style(container::rounded_box),
                );
            }
        }

        scrollable(container(content).width(Length::Fill)).into()
    }
}

pub fn format_duration(secs: f64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn results_with(dir: &std::path::Path) -> (ResultsState, Vec<PathBuf>) {
        let files: Vec<PathBuf> = ["a.jpg", "b.jpg", "c.jpg"]
            .iter()
            .map(|n| {
                let p = dir.join(n);
                std::fs::write(&p, n.as_bytes()).unwrap();
                p
            })
            .collect();
        let group = DuplicateGroup {
            files: files.clone(),
            similarity: 1.0,
        };
        (
            ResultsState::new(
                Some(1),
                vec![ResultGroup {
                    category: FileCategory::Image,
                    group,
                }],
            ),
            files,
        )
    }

    #[test]
    fn pause_and_resume_keep_the_same_scan() {
        let mut s = DuplicateFinderState {
            paths: vec!["/tmp".into()],
            ..Default::default()
        };
        let _ = s.update(FinderMessage::StartScan);
        let generation = s.scan_generation;
        let control = s.scan_control.clone().unwrap();
        let _ = s.update(FinderMessage::PauseScan);
        assert!(
            s.scan_state.is_active(),
            "paused scans must stay subscribed"
        );
        assert!(control.is_paused());
        let _ = s.update(FinderMessage::ResumeScan);
        assert!(matches!(s.scan_state, ScanState::Running { .. }));
        assert_eq!(s.scan_generation, generation);
        assert!(!control.is_paused());
    }

    #[test]
    fn delete_requires_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let (results, files) = results_with(dir.path());
        let mut s = DuplicateFinderState {
            results: Some(results),
            ..Default::default()
        };
        let _ = s.update(FinderMessage::ToggleFileForDeletion(files[1].clone()));
        let _ = s.update(FinderMessage::DeleteSelected);
        assert!(s.results.as_ref().unwrap().confirm_delete);
        assert!(files[1].exists(), "nothing deleted before confirming");
        let _ = s.update(FinderMessage::CancelDelete);
        assert!(!s.results.as_ref().unwrap().confirm_delete);
    }

    #[test]
    fn keep_strategy_selects_all_but_one() {
        let dir = tempfile::tempdir().unwrap();
        let (results, _) = results_with(dir.path());
        let mut s = DuplicateFinderState {
            results: Some(results),
            ..Default::default()
        };
        let _ = s.update(FinderMessage::ApplyKeep(KeepStrategy::ShortestPath));
        assert_eq!(s.results.as_ref().unwrap().selected_for_deletion.len(), 2);
    }

    #[test]
    fn finished_deletion_updates_groups_and_requests_db_sync() {
        let dir = tempfile::tempdir().unwrap();
        let (results, files) = results_with(dir.path());
        let mut s = DuplicateFinderState {
            results: Some(results),
            ..Default::default()
        };
        let report = RemoveReport {
            removed: vec![files[0].clone(), files[1].clone()],
            failed: vec![],
            bytes_freed: 10,
        };
        let _ = s.update(FinderMessage::DeleteFinished(Arc::new(report), 0));
        // One file left -> group disappears.
        assert!(s.results.as_ref().unwrap().groups.is_empty());
        assert!(matches!(
            s.requests.last(),
            Some(FinderRequest::FilesRemoved { session_id: 1, paths }) if paths.len() == 2
        ));
    }

    #[test]
    fn cannot_start_without_types() {
        let mut s = DuplicateFinderState {
            paths: vec!["/tmp".into()],
            ..Default::default()
        };
        s.file_types = FileTypeSelection {
            image: false,
            document: false,
            video: false,
            archive: false,
            code: false,
        };
        assert!(!s.can_start());
    }
}
