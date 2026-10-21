use crate::tabs::auto_sorter::{AutoSorterState, SortState, SortSummary, SorterMessage};
use crate::tabs::duplicate_finder::{
    DuplicateFinderState, FinderMessage, FinderRequest, ResultGroup, ResultsState, SessionSummary,
};
use crate::tabs::settings::{DbAction, DownloadState, SettingsMessage, SettingsState};
use crate::views::results_viewer;
use iced::keyboard;
use iced::widget::{button, column, container, row, text, Rule};
use iced::{Element, Length, Subscription, Task, Theme};
use shingan_core::detector::archive::ArchiveDetector;
use shingan_core::detector::Detector;
use shingan_core::file_info::FileCategory;
use shingan_core::scanner::duplicate::{DuplicateScanner, ScanControl, ScanProgress};
use shingan_core::scanner::grouping::DuplicateGroup;
use shingan_db::Database;
use shingan_utils::cleanup::RemoveMode;
use shingan_utils::settings::AppSettings;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub struct App {
    active_tab: Tab,
    settings: AppSettings,
    db: Arc<Database>,
    /// Shown in the header when the database could not be opened.
    db_warning: Option<String>,
    finder: DuplicateFinderState,
    sorter: AutoSorterState,
    settings_tab: SettingsState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    DuplicateFinder,
    AutoSorter,
    Settings,
}

#[derive(Debug, Clone)]
pub enum Message {
    TabSelected(Tab),
    ThemeToggled,
    Finder(FinderMessage),
    Sorter(SorterMessage),
    Settings(SettingsMessage),
    KeyPressed(keyboard::Key, keyboard::Modifiers),
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        let settings = shingan_utils::settings::load_settings();
        let (db, db_warning) = match Database::open(None) {
            Ok(db) => (db, None),
            Err(e) => (
                Database::open(Some(std::path::Path::new(":memory:")))
                    .expect("in-memory database must succeed"),
                Some(format!(
                    "Could not open the database ({e}); scan history won't be saved this session."
                )),
            ),
        };

        let mut app = Self {
            active_tab: Tab::DuplicateFinder,
            settings_tab: SettingsState::from_settings(&settings),
            settings,
            db: Arc::new(db),
            db_warning,
            finder: DuplicateFinderState::default(),
            sorter: AutoSorterState::default(),
        };
        // Folders given on the command line (e.g. `shingan-gui ~/Pictures`) are
        // pre-added to both tabs.
        for arg in std::env::args_os().skip(1) {
            let p = PathBuf::from(arg);
            if p.is_dir() {
                let s = p.to_string_lossy().into_owned();
                app.finder.paths.push(s.clone());
                app.sorter.source_paths.push(s);
            }
        }
        app.apply_settings();
        app.refresh_sessions();
        (app, Task::none())
    }

    pub fn title(&self) -> String {
        "shingan — duplicate finder & file organizer".to_string()
    }

    pub fn theme(&self) -> Theme {
        if self.settings.dark_theme {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    /// Push settings into the tabs that depend on them.
    fn apply_settings(&mut self) {
        self.finder.dark = self.settings.dark_theme;
        self.finder.remove_mode = if self.settings.delete_permanently {
            RemoveMode::Permanent
        } else {
            RemoveMode::Trash
        };
        self.sorter.clip_installed = self.settings_tab.clip_installed();
        self.sorter.clip_problem = if self.sorter.clip_installed {
            let dir = shingan_ml::model_registry::resolve_models_dir(
                self.settings.ml_model_path.as_deref().unwrap_or(""),
            );
            shingan_ml::onnx::ensure_runtime(&dir).err()
        } else {
            None
        };
        self.sorter.cloud_enabled = self.settings.cloud_enabled;
    }

    fn refresh_sessions(&mut self) {
        let sessions = self.db.list_scan_sessions().unwrap_or_default();
        self.finder.recent_sessions = sessions
            .into_iter()
            .map(|s| {
                let (groups, files) = self.db.session_counts(s.id).unwrap_or((0, 0));
                SessionSummary {
                    id: s.id,
                    name: s.name,
                    created_at: s.created_at,
                    status: s.status,
                    groups,
                    files,
                }
            })
            .collect();
    }

    fn load_session(&mut self, id: i64) -> Task<FinderMessage> {
        let Ok(data) = self.db.get_session_files(id) else {
            self.finder.status_message = Some(format!("Could not load scan #{id}."));
            return Task::none();
        };
        let groups: Vec<ResultGroup> = data
            .into_iter()
            .filter_map(|(g, entries)| {
                let category = FileCategory::from_label(&g.file_type)?;
                let files: Vec<PathBuf> = entries
                    .into_iter()
                    .map(|e| PathBuf::from(e.file_path))
                    .collect();
                (files.len() >= 2).then_some(ResultGroup {
                    category,
                    group: DuplicateGroup {
                        files,
                        similarity: g.similarity_score,
                    },
                })
            })
            .collect();
        let mut results = ResultsState::new(Some(id), groups);
        results.show_results = true;
        self.finder.current_session_id = Some(id);
        self.finder.set_results(results)
    }

    /// Carry out database work requested by the finder tab.
    fn process_finder_requests(&mut self) -> Task<FinderMessage> {
        let mut tasks = Vec::new();
        for req in std::mem::take(&mut self.finder.requests) {
            match req {
                FinderRequest::OpenSession(id) => tasks.push(self.load_session(id)),
                FinderRequest::DeleteSession(id) => {
                    let _ = self.db.delete_scan_session(id);
                    self.refresh_sessions();
                }
                FinderRequest::FilesRemoved { session_id, paths } => {
                    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                    let _ = self.db.remove_file_entries(session_id, &refs);
                    self.refresh_sessions();
                }
                FinderRequest::RefreshSessions => self.refresh_sessions(),
            }
        }
        Task::batch(tasks)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TabSelected(tab) => {
                self.active_tab = tab;
                Task::none()
            }
            Message::ThemeToggled => {
                self.settings.dark_theme = !self.settings.dark_theme;
                let _ = shingan_utils::settings::save_settings(&self.settings);
                self.apply_settings();
                Task::none()
            }
            Message::Finder(msg) => {
                match &msg {
                    FinderMessage::StartScan if self.finder.can_start() => {
                        let categories = self.finder.file_types.selected_categories();
                        let types_json = serde_json::to_string(
                            &categories.iter().map(|c| c.label()).collect::<Vec<_>>(),
                        )
                        .unwrap_or_default();
                        let name = self
                            .finder
                            .paths
                            .iter()
                            .map(|p| {
                                std::path::Path::new(p)
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| p.clone())
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        let threshold = self.finder.threshold as f64 / 100.0;
                        self.finder.current_session_id = self
                            .db
                            .create_scan_session(&name, &types_json, threshold)
                            .ok();
                        if let Some(id) = self.finder.current_session_id {
                            self.db.update_session_status(id, "running").ok();
                        }
                    }
                    FinderMessage::ScanProgress(ScanProgress::PhaseCompleted {
                        category,
                        groups,
                    }) => {
                        if let Some(session_id) = self.finder.current_session_id {
                            self.persist_groups(session_id, *category, groups);
                        }
                    }
                    FinderMessage::ScanProgress(ScanProgress::Completed)
                        if self.finder.scan_state.is_active() =>
                    {
                        if let Some(id) = self.finder.current_session_id {
                            self.db.update_session_status(id, "completed").ok();
                        }
                    }
                    FinderMessage::StopScan => {
                        if let Some(id) = self.finder.current_session_id {
                            self.db.update_session_status(id, "stopped").ok();
                        }
                    }
                    _ => {}
                }
                let task = self.finder.update(msg);
                let extra = self.process_finder_requests();
                Task::batch([task, extra]).map(Message::Finder)
            }
            Message::KeyPressed(key, modifiers) => self.on_key(key, modifiers),
            Message::Sorter(msg) => self.sorter.update(msg).map(Message::Sorter),
            Message::Settings(msg) => {
                let task = self.settings_tab.update(msg, &self.settings);
                if let Some(saved) = self.settings_tab.saved.take() {
                    self.settings = saved;
                }
                if let Some(action) = self.settings_tab.pending_action.take() {
                    self.settings_tab.status_message = Some(match action {
                        DbAction::ClearSessions => match self.db.clear_sessions() {
                            Ok(n) => format!("Deleted {n} scans from the history."),
                            Err(e) => format!("Error: {e}"),
                        },
                        DbAction::ClearSignatureCache => match self.db.clear_signature_cache() {
                            Ok(n) => format!("Forgot {n} cached fingerprints."),
                            Err(e) => format!("Error: {e}"),
                        },
                        DbAction::Optimize => match self.db.vacuum() {
                            Ok(()) => "Database compacted.".to_string(),
                            Err(e) => format!("Error: {e}"),
                        },
                    });
                    if action == DbAction::ClearSessions {
                        self.finder.results = None;
                        self.refresh_sessions();
                    }
                }
                self.apply_settings();
                task.map(Message::Settings)
            }
        }
    }

    fn on_key(&mut self, key: keyboard::Key, modifiers: keyboard::Modifiers) -> Task<Message> {
        use keyboard::key::Named;
        if let keyboard::Key::Named(Named::Escape) = key.as_ref() {
            if let Some(results) = self.finder.results.as_ref() {
                let msg = if results.preview.is_some() {
                    FinderMessage::ClosePreview
                } else if results.confirm_delete {
                    FinderMessage::CancelDelete
                } else if results.show_results {
                    FinderMessage::CloseResults
                } else {
                    return Task::none();
                };
                return self.update(Message::Finder(msg));
            }
            return Task::none();
        }
        if !modifiers.command() {
            return Task::none();
        }
        match key.as_ref() {
            keyboard::Key::Character("1") => self.active_tab = Tab::DuplicateFinder,
            keyboard::Key::Character("2") => self.active_tab = Tab::AutoSorter,
            keyboard::Key::Character("3") => self.active_tab = Tab::Settings,
            keyboard::Key::Character("t") => return self.update(Message::ThemeToggled),
            keyboard::Key::Named(Named::Enter)
                if self.active_tab == Tab::DuplicateFinder && self.finder.can_start() =>
            {
                return self.update(Message::Finder(FinderMessage::StartScan));
            }
            _ => {}
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        if let Some(results) = &self.finder.results {
            if results.show_results && self.active_tab == Tab::DuplicateFinder {
                return results_viewer::view(results, self.finder.remove_mode).map(Message::Finder);
            }
        }

        let mut header = row![
            text("shingan").size(24),
            text("find duplicates · sort files")
                .size(14)
                .style(text::secondary),
            iced::widget::horizontal_space(),
            button(
                text(if self.settings.dark_theme {
                    "Light mode"
                } else {
                    "Dark mode"
                })
                .size(13)
            )
            .padding([4, 12])
            .style(button::secondary)
            .on_press(Message::ThemeToggled),
        ]
        .spacing(10)
        .padding([12, 20])
        .align_y(iced::Alignment::Center);
        if let Some(w) = &self.db_warning {
            header = header.push(text(w).size(12).style(text::danger));
        }

        let tab_bar = row![
            tab_button(
                "Find duplicates",
                "Ctrl+1",
                Tab::DuplicateFinder,
                self.active_tab
            ),
            tab_button("Auto-sort", "Ctrl+2", Tab::AutoSorter, self.active_tab),
            tab_button("Settings", "Ctrl+3", Tab::Settings, self.active_tab),
        ]
        .spacing(4)
        .padding([0, 20]);

        let tab_content: Element<Message> = match self.active_tab {
            Tab::DuplicateFinder => self.finder.view().map(Message::Finder),
            Tab::AutoSorter => self.sorter.view().map(Message::Sorter),
            Tab::Settings => self.settings_tab.view().map(Message::Settings),
        };

        container(
            column![
                header,
                Rule::horizontal(1),
                tab_bar,
                Rule::horizontal(1),
                tab_content
            ]
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = Vec::new();

        // Scanner: stays subscribed while paused so pausing never restarts the scan.
        if self.finder.scan_state.is_active() {
            if let Some(control) = &self.finder.scan_control {
                let job = ScanJob {
                    paths: self
                        .finder
                        .paths
                        .iter()
                        .map(|p| (PathBuf::from(p), self.finder.include_subdirs))
                        .collect(),
                    categories: self.finder.file_types.selected_categories(),
                    threshold: self.finder.threshold as f64 / 100.0,
                    control: control.clone(),
                    db: self.db.clone(),
                    settings: self.settings.clone(),
                };
                subs.push(
                    Subscription::run_with_id(
                        ("scanner", self.finder.scan_generation),
                        scan_subscription(job),
                    )
                    .map(|p| Message::Finder(FinderMessage::ScanProgress(p))),
                );
            }
        }

        if let DownloadState::Downloading { ref model_id, .. } = self.settings_tab.download {
            let target_dir =
                shingan_ml::model_registry::resolve_models_dir(&self.settings_tab.ml_model_path);
            subs.push(
                Subscription::run_with_id(
                    ("model-download", self.settings_tab.download_generation),
                    download_subscription(
                        model_id.clone(),
                        target_dir,
                        self.settings_tab.download_cancel.clone(),
                    ),
                )
                .map(Message::Settings),
            );
        }

        if let SortState::Running { .. } = &self.sorter.sort_state {
            let job = SortJob {
                sources: self.sorter.source_paths.iter().map(PathBuf::from).collect(),
                destination: PathBuf::from(self.sorter.destination.trim()),
                use_ml: self.sorter.use_ml,
                dry_run: self.sorter.dry_run,
                include_unknown: !self.sorter.skip_unknown,
                cancel: self.sorter.cancel.clone(),
                settings: self.settings.clone(),
            };
            subs.push(
                Subscription::run_with_id(
                    ("sorter", self.sorter.run_generation),
                    sort_subscription(job),
                )
                .map(Message::Sorter),
            );
        }

        subs.push(keyboard::on_key_press(|key, modifiers| {
            let escape = matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape));
            (modifiers.command() || escape).then_some(Message::KeyPressed(key, modifiers))
        }));

        Subscription::batch(subs)
    }

    fn persist_groups(&self, session_id: i64, category: FileCategory, groups: &[DuplicateGroup]) {
        struct OwnedEntry {
            file_path: String,
            file_size: i64,
            modified: Option<String>,
        }
        let owned: Vec<(f64, Vec<OwnedEntry>)> = groups
            .iter()
            .map(|group| {
                let entries = group
                    .files
                    .iter()
                    .map(|file_path| {
                        let meta = std::fs::metadata(file_path).ok();
                        OwnedEntry {
                            file_path: file_path.to_string_lossy().into_owned(),
                            file_size: meta.as_ref().map(|m| m.len() as i64).unwrap_or(0),
                            modified: meta
                                .and_then(|m| m.modified().ok())
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_secs().to_string()),
                        }
                    })
                    .collect();
                (group.similarity, entries)
            })
            .collect();

        let batches: Vec<(f64, Vec<shingan_db::models::NewFileEntryBatch>)> = owned
            .iter()
            .map(|(sim, entries)| {
                (
                    *sim,
                    entries
                        .iter()
                        .map(|e| shingan_db::models::NewFileEntryBatch {
                            file_path: &e.file_path,
                            file_size: e.file_size,
                            modified_time: e.modified.as_deref(),
                            thumbnail_path: None,
                            file_metadata: None,
                        })
                        .collect(),
                )
            })
            .collect();
        let slices: Vec<(
            &str,
            f64,
            Option<&str>,
            &[shingan_db::models::NewFileEntryBatch],
        )> = batches
            .iter()
            .map(|(sim, e)| (category.label(), *sim, None, e.as_slice()))
            .collect();

        if let Err(e) = self.db.insert_duplicate_groups_batch(session_id, &slices) {
            eprintln!("Failed to persist duplicate groups: {e}");
        }
    }
}

fn tab_button<'a>(
    label: &'a str,
    shortcut: &'a str,
    tab: Tab,
    active: Tab,
) -> Element<'a, Message> {
    let content = column![
        text(label).size(14),
        text(shortcut).size(10).style(text::secondary)
    ]
    .align_x(iced::Alignment::Center);
    let btn = button(content).padding([6, 20]);
    if tab == active {
        btn.style(button::primary).into()
    } else {
        btn.style(button::secondary)
            .on_press(Message::TabSelected(tab))
            .into()
    }
}

/// Everything a scan needs, captured when it starts.
struct ScanJob {
    paths: Vec<(PathBuf, bool)>,
    categories: Vec<FileCategory>,
    threshold: f64,
    control: Arc<ScanControl>,
    db: Arc<Database>,
    settings: AppSettings,
}

fn build_detectors(job: &ScanJob) -> HashMap<FileCategory, Box<dyn Detector>> {
    use shingan_core::detector::{code, document, image, video};
    let t = job.threshold;
    job.categories
        .iter()
        .map(|cat| {
            let d: Box<dyn Detector> = match cat {
                FileCategory::Archive => Box::new(ArchiveDetector::new(t)),
                FileCategory::Image => Box::new(image::ImageDetector::new(t, 12)),
                FileCategory::Code => Box::new(code::CodeDetector::new(t)),
                FileCategory::Document => Box::new(document::DocumentDetector::new(t)),
                FileCategory::Video => Box::new(video::VideoDetector::with_sampling(
                    t,
                    job.settings.video_skip_secs,
                    job.settings.video_duration_secs,
                )),
            };
            (*cat, d)
        })
        .collect()
}

/// Run the scanner on a blocking thread and stream its progress.
fn scan_subscription(job: ScanJob) -> impl futures::Stream<Item = ScanProgress> {
    iced::stream::channel(100, move |mut output| async move {
        use futures::SinkExt;

        let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
        let scanner = DuplicateScanner::new(
            &job.categories,
            build_detectors(&job),
            job.threshold,
            job.control.clone(),
            progress_tx,
        )
        .with_signature_cache(Box::new(job.db.clone()))
        .with_size_limits(job.settings.min_file_size_kb * 1024, None)
        .with_threads(job.settings.thread_count as usize);

        let db = job.db.clone();
        let paths = job.paths.clone();
        tokio::task::spawn_blocking(move || {
            let (_results, new_sigs) = scanner.scan_paths(&paths);
            if !new_sigs.is_empty() {
                if let Err(e) = db.cache_new_signatures(&new_sigs) {
                    eprintln!("Failed to cache signatures: {e}");
                }
            }
        });

        // Poll without blocking the async runtime so iced keeps redrawing.
        loop {
            match progress_rx.try_recv() {
                Ok(msg) => {
                    let done = matches!(msg, ScanProgress::Completed);
                    let _ = output.send(msg).await;
                    if done {
                        break;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    let _ = output.send(ScanProgress::Completed).await;
                    break;
                }
            }
        }
        // Keep the stream alive (idle) so the subscription doesn't restart.
        futures::future::pending::<()>().await;
    })
}

/// Download the model (and ONNX Runtime if needed) on a blocking thread.
fn download_subscription(
    model_id: String,
    target_dir: PathBuf,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> impl futures::Stream<Item = SettingsMessage> {
    iced::stream::channel(100, move |mut output| async move {
        use futures::SinkExt;

        let (tx, rx) = crossbeam_channel::unbounded::<SettingsMessage>();
        tokio::task::spawn_blocking(move || {
            let Some(model) = shingan_ml::model_registry::find_model(&model_id) else {
                let _ = tx.send(SettingsMessage::DownloadComplete(Err(
                    "unknown model".into()
                )));
                return;
            };
            let need_runtime = shingan_ml::onnx::ensure_runtime(&target_dir).is_err();
            let last = std::sync::Mutex::new(std::time::Instant::now());
            let progress_tx = tx.clone();
            let result = shingan_ml::model_registry::download_model_blocking(
                model,
                &target_dir,
                need_runtime,
                &|name, index, count, done, total| {
                    let mut last = last.lock().unwrap();
                    if last.elapsed() >= std::time::Duration::from_millis(150) || done == total {
                        *last = std::time::Instant::now();
                        let _ = progress_tx.send(SettingsMessage::DownloadProgress {
                            current_file: name.to_string(),
                            file_index: index,
                            file_count: count,
                            downloaded: done,
                            total,
                        });
                    }
                },
                &cancel,
            );
            let _ = tx.send(SettingsMessage::DownloadComplete(result));
        });

        loop {
            match rx.try_recv() {
                Ok(msg) => {
                    let done = matches!(msg, SettingsMessage::DownloadComplete(_));
                    let _ = output.send(msg).await;
                    if done {
                        break;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }
        futures::future::pending::<()>().await;
    })
}

struct SortJob {
    sources: Vec<PathBuf>,
    destination: PathBuf,
    use_ml: bool,
    dry_run: bool,
    include_unknown: bool,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    settings: AppSettings,
}

fn sort_subscription(job: SortJob) -> impl futures::Stream<Item = SorterMessage> {
    iced::stream::channel(100, move |mut output| async move {
        use futures::SinkExt;

        let (tx, rx) = crossbeam_channel::unbounded::<SorterMessage>();
        tokio::task::spawn_blocking(move || {
            let mut notes = Vec::new();
            let cloud = if job.use_ml {
                match job.settings.cloud_categorizer() {
                    Ok(c) => c,
                    Err(e) => {
                        notes.push(format!("Cloud classification disabled: {e}"));
                        None
                    }
                }
            } else {
                None
            };
            let mut sorter =
                shingan_utils::auto_sorter::AutoSorter::new(job.sources, job.destination)
                    .with_ml(job.use_ml)
                    .with_dry_run(job.dry_run)
                    .with_include_unknown(job.include_unknown)
                    .with_pipeline_config(job.settings.pipeline_config())
                    .with_cloud(cloud);
            if let Some(c) = job.cancel {
                sorter = sorter.with_cancel_flag(c);
            }
            let collected = std::sync::Mutex::new(Vec::<String>::new());
            let progress_tx = tx.clone();
            let stats = sorter.sort_files(
                Some(&|current, total, file| {
                    let _ = progress_tx.send(SorterMessage::SortProgress {
                        current,
                        total,
                        file: file.to_string(),
                    });
                }),
                Some(&|msg| {
                    // Per-file dry-run lines are shown from `moves` instead.
                    if !msg.starts_with("  ")
                        && !msg.starts_with("Sorting ")
                        && !msg.starts_with("Found ")
                    {
                        collected.lock().unwrap().push(msg.to_string());
                    }
                }),
            );
            notes.extend(collected.into_inner().unwrap_or_default());
            let _ = tx.send(SorterMessage::SortCompleted(Box::new(SortSummary {
                dry_run: job.dry_run,
                moved: stats.moved,
                failed: stats.failed,
                skipped: stats.skipped,
                cancelled: stats.cancelled,
                moves: stats.moves,
                errors: stats.errors,
                notes,
                journal: stats.journal,
                undo_message: None,
            })));
        });

        loop {
            match rx.try_recv() {
                Ok(msg) => {
                    let done = matches!(msg, SorterMessage::SortCompleted(_));
                    let _ = output.send(msg).await;
                    if done {
                        break;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }
        futures::future::pending::<()>().await;
    })
}
