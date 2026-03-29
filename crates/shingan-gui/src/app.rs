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
                        scan_subscription(paths, categories, threshold, control),
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

            subs.push(
                Subscription::run_with_id(
                    "sorter",
                    sort_subscription(sources, dest),
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
        for group in groups {
            let group_id = match self.db.insert_duplicate_group(
                &shingan_db::models::NewDuplicateGroup {
                    session_id,
                    file_type: category.label(),
                    similarity_score: group.similarity,
                    hash_value: None,
                },
            ) {
                Ok(id) => id,
                Err(_) => continue,
            };

            for file_path in &group.files {
                let meta = std::fs::metadata(file_path).ok();
                let size = meta.as_ref().map(|m| m.len() as i64).unwrap_or(0);

                self.db
                    .insert_file_entry(&shingan_db::models::NewFileEntry {
                        group_id,
                        file_path: &file_path.to_string_lossy(),
                        file_size: size,
                        modified_time: None,
                        thumbnail_path: None,
                        file_metadata: None,
                    })
                    .ok();
            }
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

        let scanner = DuplicateScanner::new(
            &categories,
            detectors,
            threshold,
            control,
            progress_tx,
        );

        // Run scanner on blocking thread
        let paths_clone = paths.clone();
        tokio::task::spawn_blocking(move || {
            scanner.scan_paths(&paths_clone);
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
fn sort_subscription(
    sources: Vec<PathBuf>,
    destination: PathBuf,
) -> impl futures::Stream<Item = SorterMessage> {
    iced::stream::channel(100, move |mut output| async move {
        use futures::SinkExt;

        let (progress_tx, progress_rx) = crossbeam_channel::unbounded::<SorterMessage>();

        let tx = progress_tx.clone();
        tokio::task::spawn_blocking(move || {
            let sorter =
                shingan_utils::auto_sorter::AutoSorter::new(sources, destination);
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
