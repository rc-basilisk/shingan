use iced::widget::{button, checkbox, column, container, progress_bar, row, scrollable, text, text_input, Rule};
use iced::{Element, Length, Task};

/// State for the Auto-Sorter tab.
pub struct AutoSorterState {
    pub source_paths: Vec<String>,
    pub destination: String,
    pub use_ml: bool,
    pub sort_state: SortState,
}

#[derive(Debug, Clone)]
pub enum SortState {
    Idle,
    Running { progress: f32, status: String },
    Completed { moved: u64, failed: u64 },
}

#[derive(Debug, Clone)]
pub enum SorterMessage {
    AddSource,
    SourceSelected(Option<String>),
    RemoveSource(usize),
    SelectDestination,
    DestinationSelected(Option<String>),
    DestinationChanged(String),
    ToggleML(bool),
    StartSorting,
    SortProgress { current: u64, total: u64, file: String },
    SortCompleted { moved: u64, failed: u64 },
}

impl Default for AutoSorterState {
    fn default() -> Self {
        Self {
            source_paths: Vec::new(),
            destination: String::new(),
            use_ml: false,
            sort_state: SortState::Idle,
        }
    }
}

impl AutoSorterState {
    pub fn update(&mut self, message: SorterMessage) -> Task<SorterMessage> {
        match message {
            SorterMessage::AddSource => {
                return Task::perform(
                    async {
                        let handle = rfd::AsyncFileDialog::new().pick_folder().await;
                        handle.map(|h| h.path().to_string_lossy().to_string())
                    },
                    SorterMessage::SourceSelected,
                );
            }
            SorterMessage::SourceSelected(Some(path)) => {
                if !self.source_paths.contains(&path) {
                    self.source_paths.push(path);
                }
            }
            SorterMessage::SourceSelected(None) => {}
            SorterMessage::RemoveSource(idx) => {
                if idx < self.source_paths.len() {
                    self.source_paths.remove(idx);
                }
            }
            SorterMessage::SelectDestination => {
                return Task::perform(
                    async {
                        let handle = rfd::AsyncFileDialog::new().pick_folder().await;
                        handle.map(|h| h.path().to_string_lossy().to_string())
                    },
                    SorterMessage::DestinationSelected,
                );
            }
            SorterMessage::DestinationSelected(Some(path)) => {
                self.destination = path;
            }
            SorterMessage::DestinationSelected(None) => {}
            SorterMessage::DestinationChanged(val) => {
                self.destination = val;
            }
            SorterMessage::ToggleML(val) => {
                self.use_ml = val;
            }
            SorterMessage::StartSorting => {
                self.sort_state = SortState::Running {
                    progress: 0.0,
                    status: "Starting...".to_string(),
                };
            }
            SorterMessage::SortProgress { current, total, file } => {
                self.sort_state = SortState::Running {
                    progress: if total > 0 { current as f32 / total as f32 } else { 0.0 },
                    status: format!("{}/{}: {}", current, total, file),
                };
            }
            SorterMessage::SortCompleted { moved, failed } => {
                self.sort_state = SortState::Completed { moved, failed };
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, SorterMessage> {
        let mut content = column![].spacing(12).padding([16, 24]);

        // -- Source Folders --
        content = content.push(text("Source Folders").size(16));
        for (i, path) in self.source_paths.iter().enumerate() {
            content = content.push(
                container(
                    row![
                        text(path).size(13).width(Length::Fill),
                        button(text("Remove").size(12))
                            .padding([4, 10])
                            .on_press(SorterMessage::RemoveSource(i)),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                )
                .padding([6, 12]),
            );
        }
        content = content.push(button("Add Folder").on_press(SorterMessage::AddSource));

        // -- Destination --
        content = content.push(Rule::horizontal(1));
        content = content.push(text("Destination Folder").size(16));
        content = content.push(
            row![
                text_input("Select destination folder...", &self.destination)
                    .on_input(SorterMessage::DestinationChanged)
                    .width(Length::Fill),
                button(text("Browse").size(13))
                    .padding([6, 14])
                    .on_press(SorterMessage::SelectDestination),
            ]
            .spacing(10),
        );

        // -- Options --
        content = content.push(Rule::horizontal(1));
        content = content.push(
            checkbox("Sort images into sub-categories (local ML)", self.use_ml)
                .on_toggle(SorterMessage::ToggleML),
        );

        // -- Control --
        content = content.push(Rule::horizontal(1));
        match &self.sort_state {
            SortState::Idle => {
                let mut start = button(text("Start Sorting").size(14)).padding([8, 24]);
                if !self.source_paths.is_empty() && !self.destination.is_empty() {
                    start = start.on_press(SorterMessage::StartSorting);
                }
                content = content.push(start);
            }
            SortState::Running { progress, status } => {
                content = content.push(
                    container(
                        column![
                            progress_bar(0.0..=1.0, *progress).height(20),
                            text(status).size(13),
                        ]
                        .spacing(6),
                    )
                    .padding([10, 0]),
                );
            }
            SortState::Completed { moved, failed } => {
                content = content.push(
                    container(
                        text(format!(
                            "Sorting complete! {} files moved, {} failed",
                            moved, failed
                        ))
                        .size(14),
                    )
                    .padding([10, 14]),
                );
                content = content.push(
                    button(text("Start Sorting").size(14))
                        .padding([8, 24])
                        .on_press(SorterMessage::StartSorting),
                );
            }
        }

        scrollable(container(content).width(Length::Fill)).into()
    }
}
