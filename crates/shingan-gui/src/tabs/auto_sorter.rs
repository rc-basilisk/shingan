use iced::widget::{
    button, checkbox, column, container, progress_bar, row, scrollable, text, text_input, Rule,
};
use iced::{Element, Length, Task};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// State for the Auto-Sorter tab.
pub struct AutoSorterState {
    pub source_paths: Vec<String>,
    pub destination: String,
    pub use_ml: bool,
    pub dry_run: bool,
    pub skip_unknown: bool,
    pub sort_state: SortState,
    /// Bumped per run so the subscription restarts for every sort.
    pub run_generation: u64,
    pub cancel: Option<Arc<AtomicBool>>,
    /// Hints from the app: whether the CLIP model / cloud are available.
    pub clip_installed: bool,
    /// Why the installed model can't be used (e.g. ONNX Runtime missing).
    pub clip_problem: Option<String>,
    pub cloud_enabled: bool,
}

#[derive(Debug, Clone)]
pub enum SortState {
    Idle,
    Running { progress: f32, status: String },
    Completed(SortSummary),
}

/// What a finished (or previewed) sort did.
#[derive(Debug, Clone)]
pub struct SortSummary {
    pub dry_run: bool,
    pub moved: u64,
    pub failed: u64,
    pub skipped: u64,
    pub cancelled: bool,
    pub moves: Vec<(PathBuf, PathBuf)>,
    pub errors: Vec<String>,
    pub notes: Vec<String>,
    pub journal: Option<PathBuf>,
    pub undo_message: Option<String>,
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
    ToggleDryRun(bool),
    ToggleSkipUnknown(bool),
    StartSorting,
    StopSorting,
    SortProgress {
        current: u64,
        total: u64,
        file: String,
    },
    SortCompleted(Box<SortSummary>),
    Undo,
    UndoFinished(String),
    OpenDestination,
}

impl Default for AutoSorterState {
    fn default() -> Self {
        Self {
            source_paths: Vec::new(),
            destination: String::new(),
            use_ml: false,
            dry_run: true,
            skip_unknown: false,
            sort_state: SortState::Idle,
            run_generation: 0,
            cancel: None,
            clip_installed: false,
            clip_problem: None,
            cloud_enabled: false,
        }
    }
}

fn pick_folder() -> Task<Option<String>> {
    Task::perform(
        async {
            rfd::AsyncFileDialog::new()
                .pick_folder()
                .await
                .map(|h| h.path().to_string_lossy().to_string())
        },
        |p| p,
    )
}

impl AutoSorterState {
    fn can_start(&self) -> bool {
        !self.source_paths.is_empty() && !self.destination.trim().is_empty()
    }

    pub fn update(&mut self, message: SorterMessage) -> Task<SorterMessage> {
        match message {
            SorterMessage::AddSource => return pick_folder().map(SorterMessage::SourceSelected),
            SorterMessage::SourceSelected(Some(path)) => {
                if !self.source_paths.contains(&path) {
                    self.source_paths.push(path);
                }
            }
            SorterMessage::SourceSelected(None) | SorterMessage::DestinationSelected(None) => {}
            SorterMessage::RemoveSource(idx) => {
                if idx < self.source_paths.len() {
                    self.source_paths.remove(idx);
                }
            }
            SorterMessage::SelectDestination => {
                return pick_folder().map(SorterMessage::DestinationSelected)
            }
            SorterMessage::DestinationSelected(Some(path)) => self.destination = path,
            SorterMessage::DestinationChanged(val) => self.destination = val,
            SorterMessage::ToggleML(val) => self.use_ml = val,
            SorterMessage::ToggleDryRun(val) => self.dry_run = val,
            SorterMessage::ToggleSkipUnknown(val) => self.skip_unknown = val,
            SorterMessage::StartSorting => {
                if self.can_start() {
                    self.run_generation += 1;
                    self.cancel = Some(Arc::new(AtomicBool::new(false)));
                    self.sort_state = SortState::Running {
                        progress: 0.0,
                        status: "Looking for files...".to_string(),
                    };
                }
            }
            SorterMessage::StopSorting => {
                if let Some(c) = &self.cancel {
                    c.store(true, Ordering::Relaxed);
                }
                if let SortState::Running { status, .. } = &mut self.sort_state {
                    *status = "Stopping after the current file...".to_string();
                }
            }
            SorterMessage::SortProgress {
                current,
                total,
                file,
            } => {
                if let SortState::Running { progress, status } = &mut self.sort_state {
                    *progress = if total > 0 {
                        current as f32 / total as f32
                    } else {
                        0.0
                    };
                    let name = std::path::Path::new(&file)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or(file);
                    *status = format!("{current}/{total}: {name}");
                }
            }
            SorterMessage::SortCompleted(summary) => {
                self.cancel = None;
                self.sort_state = SortState::Completed(*summary);
            }
            SorterMessage::Undo => {
                if let SortState::Completed(SortSummary {
                    journal: Some(journal),
                    undo_message,
                    ..
                }) = &mut self.sort_state
                {
                    *undo_message = Some("Restoring files...".to_string());
                    let journal = journal.clone();
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                match shingan_utils::auto_sorter::undo_sort(&journal) {
                                    Ok(s) if s.failed == 0 => {
                                        format!("Undone: {} files restored.", s.restored)
                                    }
                                    Ok(s) => format!(
                                        "{} files restored, {} could not be ({})",
                                        s.restored,
                                        s.failed,
                                        s.errors.first().cloned().unwrap_or_default()
                                    ),
                                    Err(e) => format!("Undo failed: {e}"),
                                }
                            })
                            .await
                            .unwrap_or_else(|e| format!("Undo failed: {e}"))
                        },
                        SorterMessage::UndoFinished,
                    );
                }
            }
            SorterMessage::UndoFinished(msg) => {
                if let SortState::Completed(summary) = &mut self.sort_state {
                    summary.undo_message = Some(msg);
                    summary.journal = None;
                }
            }
            SorterMessage::OpenDestination => {
                crate::platform::open_path(std::path::Path::new(&self.destination));
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, SorterMessage> {
        let running = matches!(self.sort_state, SortState::Running { .. });
        let mut content = column![].spacing(12).padding([16, 24]);

        content = content.push(
            text("Move files into folders by type: images/, documents/, videos/, archives/, code/ and others/.")
                .size(13)
                .style(text::secondary),
        );

        // -- Source Folders --
        content = content.push(text("Folders to sort").size(16));
        if self.source_paths.is_empty() {
            content = content.push(
                text("No folders yet — add one (for example your Downloads folder).")
                    .size(13)
                    .style(text::secondary),
            );
        }
        for (i, path) in self.source_paths.iter().enumerate() {
            let mut remove = button(text("Remove").size(12))
                .padding([4, 10])
                .style(button::secondary);
            if !running {
                remove = remove.on_press(SorterMessage::RemoveSource(i));
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
        if !running {
            add = add.on_press(SorterMessage::AddSource);
        }
        content = content.push(add);

        // -- Destination --
        content = content.push(Rule::horizontal(1));
        content = content.push(text("Destination").size(16));
        let mut browse = button(text("Browse").size(13)).padding([6, 14]);
        if !running {
            browse = browse.on_press(SorterMessage::SelectDestination);
        }
        let mut dest_input =
            text_input("Where the sorted folders should go", &self.destination).width(Length::Fill);
        if !running {
            dest_input = dest_input.on_input(SorterMessage::DestinationChanged);
        }
        content = content.push(row![dest_input, browse].spacing(10));
        content = content.push(
            text("The destination may be inside a source folder; already-sorted files are left alone.")
                .size(12)
                .style(text::secondary),
        );

        // -- Options --
        content = content.push(Rule::horizontal(1));
        let mut opts = column![
            checkbox("Preview only (don't move anything)", self.dry_run)
                .on_toggle_maybe((!running).then_some(SorterMessage::ToggleDryRun)),
            checkbox(
                "Leave files of unknown types where they are (instead of others/)",
                self.skip_unknown
            )
            .on_toggle_maybe((!running).then_some(SorterMessage::ToggleSkipUnknown)),
            checkbox(
                "Sort images into sub-folders (photos, screenshots, memes, ...)",
                self.use_ml
            )
            .on_toggle_maybe((!running).then_some(SorterMessage::ToggleML)),
        ]
        .spacing(8);
        if self.use_ml && self.clip_installed && self.clip_problem.is_some() {
            opts = opts.push(
                text(format!(
                    "The local model is installed but can't run: {}",
                    self.clip_problem.as_deref().unwrap_or_default()
                ))
                .size(12)
                .style(text::danger),
            );
        } else if self.use_ml {
            let hint = match (self.clip_installed, self.cloud_enabled) {
                (true, true) => "Using the local model, plus the cloud provider for uncertain images.",
                (true, false) => "Using the local model.",
                (false, true) => "Local model not installed: using simple heuristics and the cloud provider.",
                (false, false) => "Local model not installed: only simple heuristics (screenshots, icons, camera photos). Download it in Settings for much better results.",
            };
            opts = opts.push(text(hint).size(12).style(text::secondary));
        }
        content = content.push(opts);

        // -- Control --
        content = content.push(Rule::horizontal(1));
        let start_label = if self.dry_run {
            "Preview sort"
        } else {
            "Sort files"
        };
        match &self.sort_state {
            SortState::Running { progress, status } => {
                content = content.push(
                    column![
                        progress_bar(0.0..=1.0, *progress).height(20),
                        row![
                            text(status).size(13).width(Length::Fill),
                            button(text("Stop").size(13))
                                .padding([6, 16])
                                .style(button::danger)
                                .on_press(SorterMessage::StopSorting),
                        ]
                        .spacing(10)
                        .align_y(iced::Alignment::Center),
                    ]
                    .spacing(6),
                );
            }
            state => {
                let mut start = button(text(start_label).size(14))
                    .padding([8, 24])
                    .style(button::primary);
                if self.can_start() {
                    start = start.on_press(SorterMessage::StartSorting);
                }
                content = content.push(start);
                if let SortState::Completed(summary) = state {
                    content = content.push(self.view_summary(summary));
                }
            }
        }

        scrollable(container(content).width(Length::Fill)).into()
    }

    fn view_summary<'a>(&'a self, s: &'a SortSummary) -> Element<'a, SorterMessage> {
        let headline = if s.dry_run {
            format!(
                "Preview: {} files would be moved, {} skipped. Untick \"Preview only\" to do it.",
                s.moved, s.skipped
            )
        } else {
            format!(
                "{}: {} moved, {} failed, {} skipped.",
                if s.cancelled { "Stopped" } else { "Done" },
                s.moved,
                s.failed,
                s.skipped
            )
        };
        let mut col = column![text(headline).size(14)].spacing(8);

        for n in &s.notes {
            col = col.push(text(n).size(12).style(text::secondary));
        }

        let mut actions = row![].spacing(10);
        if !s.dry_run && s.moved > 0 {
            actions = actions.push(
                button(text("Open destination").size(13))
                    .padding([6, 14])
                    .style(button::secondary)
                    .on_press(SorterMessage::OpenDestination),
            );
        }
        if s.journal.is_some() {
            actions = actions.push(
                button(text("Undo this sort").size(13))
                    .padding([6, 14])
                    .style(button::secondary)
                    .on_press(SorterMessage::Undo),
            );
        }
        col = col.push(actions);
        if let Some(m) = &s.undo_message {
            col = col.push(text(m).size(13));
        }

        if !s.errors.is_empty() {
            col = col.push(text(format!("Problems ({}):", s.errors.len())).size(13));
            for e in s.errors.iter().take(20) {
                col = col.push(text(e).size(12).style(text::danger));
            }
        }

        if s.dry_run && !s.moves.is_empty() {
            let dest = PathBuf::from(&self.destination);
            let mut list = column![].spacing(2);
            for (from, to) in s.moves.iter().take(300) {
                let rel = to.strip_prefix(&dest).unwrap_or(to);
                let from = self
                    .source_paths
                    .iter()
                    .find_map(|src| from.strip_prefix(src).ok())
                    .unwrap_or(from);
                list = list.push(
                    text(format!("{}  →  {}", from.display(), rel.display()))
                        .size(12)
                        .font(iced::Font::MONOSPACE),
                );
            }
            if s.moves.len() > 300 {
                list = list.push(text(format!("... and {} more", s.moves.len() - 300)).size(12));
            }
            col = col.push(
                container(scrollable(list).height(Length::Fixed(260.0)))
                    .padding(8)
                    .style(container::bordered_box),
            );
        }

        container(col).padding([10, 0]).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cannot_start_without_source_and_destination() {
        let mut s = AutoSorterState::default();
        let _ = s.update(SorterMessage::StartSorting);
        assert!(matches!(s.sort_state, SortState::Idle));
        s.source_paths.push("/tmp/a".into());
        s.destination = "/tmp/b".into();
        let _ = s.update(SorterMessage::StartSorting);
        assert!(matches!(s.sort_state, SortState::Running { .. }));
        assert_eq!(s.run_generation, 1);
    }

    #[test]
    fn stop_sets_cancel_flag() {
        let mut s = AutoSorterState {
            source_paths: vec!["/a".into()],
            destination: "/b".into(),
            ..Default::default()
        };
        let _ = s.update(SorterMessage::StartSorting);
        let flag = s.cancel.clone().unwrap();
        let _ = s.update(SorterMessage::StopSorting);
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn preview_is_the_default() {
        assert!(AutoSorterState::default().dry_run);
    }
}
