use crate::tabs::auto_sorter::{AutoSorterState, SorterMessage};
use crate::tabs::duplicate_finder::{DuplicateFinderState, FinderMessage, ScanState};
use crate::tabs::settings::{SettingsMessage, SettingsState};
use crate::theme::AppTheme;
use crate::views::results_viewer;
use shingan_core::detector::archive::ArchiveDetector;
use shingan_core::detector::Detector;
use shingan_core::file_info::FileCategory;
use shingan_core::scanner::duplicate::{DuplicateScanner, ScanControl, ScanProgress};
use shingan_db::Database;
use iced::widget::{button, column, container, row, text};
use iced::{Element, Length, Subscription, Task, Theme};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub struct App {
    active_tab: Tab,
    theme: AppTheme,
    db: Arc<Database>,
    finder: DuplicateFinderState,
    sorter: AutoSorterState,
    settings: SettingsState,
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
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        let db = Database::open(None).expect("Failed to open database");

        (
            Self {
                active_tab: Tab::DuplicateFinder,
                theme: AppTheme::default(),
                db: Arc::new(db),
                finder: DuplicateFinderState::default(),
                sorter: AutoSorterState::default(),
                settings: SettingsState::default(),
            },
            Task::none(),
        )
    }

    pub fn title(&self) -> String {
        "Shingan — File Deduplicator & Organizer".to_string()
    }

    pub fn theme(&self) -> Theme {
        self.theme.to_iced_theme()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TabSelected(tab) => {
                self.active_tab = tab;
                Task::none()
            }
            Message::ThemeToggled => {
                self.theme = self.theme.toggle();
                Task::none()
            }
            Message::Finder(msg) => {
                // Handle StartScan specially to launch the subscription
                if matches!(msg, FinderMessage::StartScan) {
                    // Create session in DB
                    let categories = self.finder.file_types.selected_categories();
                    let types_json = serde_json::to_string(
                        &categories.iter().map(|c| c.label()).collect::<Vec<_>>(),
                    )
                    .unwrap_or_default();
                    let threshold = self.finder.threshold as f64 / 100.0;

                    if let Ok(session_id) = self.db.create_scan_session(
                        &format!("Scan {} paths", self.finder.paths.len()),
                        &types_json,
                        threshold,
                    ) {
                        self.finder.current_session_id = Some(session_id);
                        self.db.update_session_status(session_id, "running").ok();
                    }
                }

                // Handle DB operations for completed scans
                if let FinderMessage::ScanProgress(ScanProgress::PhaseCompleted {
                    ref category,
                    ref groups,
                }) = msg
                {
                    if let Some(session_id) = self.finder.current_session_id {
                        self.persist_groups(session_id, *category, groups);
                    }
                }

                if let FinderMessage::ScanProgress(ScanProgress::Completed) = msg {
                    if let Some(session_id) = self.finder.current_session_id {
                        self.db.update_session_status(session_id, "completed").ok();
                    }
                }

                self.finder.update(msg).map(Message::Finder)
            }
            Message::Sorter(msg) => self.sorter.update(msg).map(Message::Sorter),
            Message::Settings(msg) => {
                // Handle DB operations from settings
                match &msg {
                    SettingsMessage::ClearSessions => {
                        let count = self.db.clear_sessions().unwrap_or(0);
                        self.settings.status_message =
                            Some(format!("Cleared {} sessions", count));
                        return Task::none();
                    }
                    SettingsMessage::OptimizeDb => {
                        match self.db.vacuum() {
                            Ok(_) => {
                                self.settings.status_message =
                                    Some("Database optimized!".to_string())
                            }
                            Err(e) => {
                                self.settings.status_message =
                                    Some(format!("Error: {}", e))
                            }
                        }
                        return Task::none();
                    }
                    _ => {}
                }
                self.settings.update(msg).map(Message::Settings)
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        // Check if results overlay should be shown
        if let Some(ref results) = self.finder.results {
            if results.show_results {
                return results_viewer::view(
                    &results.groups,
                    &results.filter,
                    &results.selected_for_deletion,
                    &results.deletion_message,
                    &results.preview,
                    results.page,
                    results.page_size,
                )
                .map(Message::Finder);
            }
        }

        let header = row![
            text("Shingan — File Deduplicator & Organizer").size(22),
            iced::widget::horizontal_space(),
            button(self.theme.toggle_label()).on_press(Message::ThemeToggled),
        ]
        .spacing(10)
        .padding(10);

        let tab_bar = row![
            tab_button("Find Duplicates", Tab::DuplicateFinder, self.active_tab),
            tab_button("Auto-Sort Files", Tab::AutoSorter, self.active_tab),
            tab_button("Settings", Tab::Settings, self.active_tab),
        ]
        .spacing(5)
        .padding([0, 10]);

        let tab_content: Element<Message> = match self.active_tab {
            Tab::DuplicateFinder => self.finder.view().map(Message::Finder),
            Tab::AutoSorter => self.sorter.view().map(Message::Sorter),
            Tab::Settings => self.settings.view().map(Message::Settings),
        };

        let content = column![header, tab_bar, tab_content]
            .spacing(5)
            .width(Length::Fill)
            .height(Length::Fill);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = Vec::new();

        // Scanner subscription
        if let ScanState::Running { .. } = &self.finder.scan_state {
            if let Some(ref control) = self.finder.scan_control {
                let paths: Vec<(PathBuf, bool)> = self
                    .finder
                    .paths
                    .iter()
                    .map(|p| (PathBuf::from(p), self.finder.include_subdirs))
                    .collect();
                let categories = self.finder.file_types.selected_categories();
                let threshold = self.finder.threshold as f64 / 100.0;
                let control = control.clone();

                subs.push(
                    Subscription::run_with_id(
                        "scanner",
                        scan_subscription(paths, categories, threshold, control, self.db.clone()),
                    )
                    .map(|progress| Message::Finder(FinderMessage::ScanProgress(progress))),
                );
            }
        }

        // Sorter subscription
        if let crate::tabs::auto_sorter::SortState::Running { .. } = &self.sorter.sort_state {
            let sources: Vec<PathBuf> =
                self.sorter.source_paths.iter().map(PathBuf::from).collect();
            let dest = PathBuf::from(&self.sorter.destination);
            let use_ml = self.sorter.use_ml;

            subs.push(
                Subscription::run_with_id(
                    "sorter",
                    sort_subscription(sources, dest, use_ml),
                )
                .map(Message::Sorter),
            );
        }

        Subscription::batch(subs)
    }

    fn persist_groups(
        &self,
        session_id: i64,
        category: FileCategory,
        groups: &[shingan_core::scanner::grouping::DuplicateGroup],
    ) {
        struct OwnedEntry {
            file_path: String,
            file_size: i64,
        }
        struct OwnedGroup {
            entries: Vec<OwnedEntry>,
            similarity: f64,
        }

        let owned: Vec<OwnedGroup> = groups
            .iter()
            .map(|group| {
                let entries: Vec<OwnedEntry> = group
                    .files
                    .iter()
                    .map(|file_path| {
                        let meta = std::fs::metadata(file_path).ok();
                        let size = meta.as_ref().map(|m| m.len() as i64).unwrap_or(0);
                        OwnedEntry {
                            file_path: file_path.to_string_lossy().into_owned(),
                            file_size: size,
                        }
                    })
                    .collect();
                OwnedGroup {
                    entries,
                    similarity: group.similarity,
                }
            })
            .collect();

        let batch_refs: Vec<(&str, f64, Option<&str>, Vec<shingan_db::models::NewFileEntryBatch>)> = owned
            .iter()
            .map(|group| {
                let entries: Vec<shingan_db::models::NewFileEntryBatch> = group
                    .entries
                    .iter()
                    .map(|e| shingan_db::models::NewFileEntryBatch {
                        file_path: &e.file_path,
                        file_size: e.file_size,
                        modified_time: None,
                        thumbnail_path: None,
                        file_metadata: None,
                    })
                    .collect();
                (category.label(), group.similarity, None as Option<&str>, entries)
            })
            .collect();

        let batch_slices: Vec<(&str, f64, Option<&str>, &[shingan_db::models::NewFileEntryBatch])> = batch_refs
            .iter()
            .map(|(ft, sim, hv, entries)| (*ft, *sim, hv.as_deref(), entries.as_slice()))
            .collect();

        if let Err(e) = self.db.insert_duplicate_groups_batch(session_id, &batch_slices) {
            eprintln!("Failed to persist duplicate groups: {e}");
        }
    }
}

fn tab_button(label: &str, tab: Tab, active: Tab) -> Element<'_, Message> {
    let btn = button(text(label).size(14));
    if tab == active {
        btn.into()
    } else {
        btn.on_press(Message::TabSelected(tab)).into()
    }
}

/// Create a stream of ScanProgress messages from a background scanner.
fn scan_subscription(
    paths: Vec<(PathBuf, bool)>,
    categories: Vec<FileCategory>,
    threshold: f64,
    control: Arc<ScanControl>,
    db: Arc<Database>,
) -> impl futures::Stream<Item = ScanProgress> {
    iced::stream::channel(100, move |mut output| async move {
        use futures::SinkExt;

        let (progress_tx, progress_rx) = crossbeam_channel::unbounded();

        // Build detectors
        let mut detectors: HashMap<FileCategory, Box<dyn Detector>> = HashMap::new();
        for cat in &categories {
            let detector: Option<Box<dyn Detector>> = match cat {
                FileCategory::Archive => Some(Box::new(ArchiveDetector::new(threshold))),
                FileCategory::Image => {
                    Some(Box::new(shingan_core::detector::image::ImageDetector::new(threshold, 12)))
                }
                FileCategory::Code => {
                    Some(Box::new(shingan_core::detector::code::CodeDetector::new(threshold)))
                }
                FileCategory::Document => {
                    Some(Box::new(shingan_core::detector::document::DocumentDetector::new(threshold)))
                }
                FileCategory::Video => {
                    Some(Box::new(shingan_core::detector::video::VideoDetector::new(threshold)))
                }
            };
            if let Some(d) = detector {
                detectors.insert(*cat, d);
            }
        }

        // Pre-load cached signatures from DB for all scan paths
        let cached_signatures = load_cached_signatures(&db, &paths, &categories);

        let scanner = DuplicateScanner::new(
            &categories,
            detectors,
            threshold,
            control,
            progress_tx,
        )
        .with_cached_signatures(cached_signatures);

        // Run scanner on blocking thread, collecting new signatures to persist
        let paths_clone = paths.clone();
        let db_clone = db.clone();
        tokio::task::spawn_blocking(move || {
            let (_results, new_sigs) = scanner.scan_paths(&paths_clone);
            // Persist newly computed signatures for future rescans
            if !new_sigs.is_empty() {
                persist_new_signatures(&db_clone, &new_sigs, &paths_clone, &categories);
            }
        });

        // Forward progress messages to the iced stream.
        // Use try_recv + async sleep so we don't block the tokio runtime
        // (blocking recv() prevents iced from processing intermediate updates).
        loop {
            match progress_rx.try_recv() {
                Ok(msg) => {
                    let is_completed = matches!(msg, ScanProgress::Completed);
                    let _ = output.send(msg).await;
                    if is_completed {
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
    })
}

/// Create a stream of SorterMessages from a background auto-sorter.
/// Walk scan paths and query DB for cached signatures of files that haven't changed.
fn load_cached_signatures(
    db: &Database,
    paths: &[(PathBuf, bool)],
    categories: &[FileCategory],
) -> HashMap<String, String> {
    use shingan_core::file_info::ExtensionMap;
    use std::collections::HashSet;

    let ext_map = ExtensionMap::new();
    let cat_set: HashSet<FileCategory> = categories.iter().copied().collect();
    let mut queries: Vec<(String, i64, i64, String)> = Vec::new();

    for (path, include_subdirs) in paths {
        let walker = if *include_subdirs {
            walkdir::WalkDir::new(path).follow_links(false).into_iter()
        } else {
            walkdir::WalkDir::new(path)
                .max_depth(1)
                .follow_links(false)
                .into_iter()
        };
        for entry in walker.filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let fpath = entry.path();
            let ext = match fpath.extension().and_then(|e| e.to_str()) {
                Some(e) => e.to_lowercase(),
                None => continue,
            };
            let cat = match ext_map.get(&ext) {
                Some(c) if cat_set.contains(&c) => c,
                _ => continue,
            };
            if let Ok(meta) = std::fs::metadata(fpath) {
                let size = meta.len() as i64;
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                queries.push((
                    fpath.to_string_lossy().into_owned(),
                    size,
                    mtime,
                    cat.label().to_string(),
                ));
            }
        }
    }

    if queries.is_empty() {
        return HashMap::new();
    }

    // Query DB in batches to avoid overly long single queries
    let query_refs: Vec<(&str, i64, i64, &str)> = queries
        .iter()
        .map(|(p, s, m, c)| (p.as_str(), *s, *m, c.as_str()))
        .collect();

    db.get_cached_signatures(&query_refs).unwrap_or_default()
}

/// Persist newly computed signatures to the DB for future rescans.
fn persist_new_signatures(
    db: &Database,
    new_sigs: &[(String, String)],
    paths: &[(PathBuf, bool)],
    _categories: &[FileCategory],
) {
    // Build a lookup of path -> (size, mtime, category) from discovered files
    let mut file_meta: HashMap<String, (i64, i64, String)> = HashMap::new();

    // We need file metadata for each signature. Walk the paths to get it.
    for (path, include_subdirs) in paths {
        let walker = if *include_subdirs {
            walkdir::WalkDir::new(path).follow_links(false).into_iter()
        } else {
            walkdir::WalkDir::new(path)
                .max_depth(1)
                .follow_links(false)
                .into_iter()
        };
        for entry in walker.filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let fpath = entry.path();
            if let Ok(meta) = std::fs::metadata(fpath) {
                let ext = fpath
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let ext_map = shingan_core::file_info::ExtensionMap::new();
                let cat = match ext_map.get(&ext) {
                    Some(c) => c.label().to_string(),
                    None => continue,
                };
                let size = meta.len() as i64;
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                file_meta.insert(fpath.to_string_lossy().into_owned(), (size, mtime, cat));
            }
        }
    }

    let entries: Vec<(&str, i64, i64, &str, &str)> = new_sigs
        .iter()
        .filter_map(|(path, sig)| {
            file_meta
                .get(path)
                .map(|(size, mtime, cat)| (path.as_str(), *size, *mtime, cat.as_str(), sig.as_str()))
        })
        .collect();

    if !entries.is_empty() {
        if let Err(e) = db.cache_signatures_batch(&entries) {
            eprintln!("Failed to cache signatures: {e}");
        }
    }
}

fn sort_subscription(
    sources: Vec<PathBuf>,
    destination: PathBuf,
    use_ml: bool,
) -> impl futures::Stream<Item = SorterMessage> {
    iced::stream::channel(100, move |mut output| async move {
        use futures::SinkExt;

        let (progress_tx, progress_rx) = crossbeam_channel::unbounded::<SorterMessage>();

        let tx = progress_tx.clone();
        tokio::task::spawn_blocking(move || {
            let sorter =
                shingan_utils::auto_sorter::AutoSorter::new(sources, destination)
                    .with_ml(use_ml);
            let stats = sorter.sort_files(
                Some(&|current, total, filepath| {
                    let _ = tx.send(SorterMessage::SortProgress {
                        current,
                        total,
                        file: filepath.to_string(),
                    });
                }),
                None,
            );
            let _ = tx.send(SorterMessage::SortCompleted {
                moved: stats.moved,
                failed: stats.failed,
            });
        });

        loop {
            match progress_rx.try_recv() {
                Ok(msg) => {
                    let is_done = matches!(msg, SorterMessage::SortCompleted { .. });
                    let _ = output.send(msg).await;
                    if is_done {
                        break;
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }
    })
}
