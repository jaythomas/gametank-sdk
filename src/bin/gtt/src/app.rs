use std::{path::{Path, PathBuf}, thread::sleep, time::Duration};

use ratatui::{
    DefaultTerminal,
    crossterm::{
        event::{
            self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
            PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::supports_keyboard_enhancement,
    },
    layout::Rect,
};

use crate::{
    action::ComponentAction,
    component::Component,
    components::{
        CommandPalette, EnvelopeEditor, ExportConfirmModal, FileBrowser, InstrumentEditor,
        PatternDeleteConfirmModal, QuitConfirmModal, TrackerContainer, TuningEditor,
    },
    config::{GttConfig, build_key_assignments, config_to_tuning_keys},
    export,
    file::{MAX_PATTERNS, NUM_INSTRUMENTS, TrackerFile},
    player::Player,
    tracker::{BeatRow, NUM_ENVELOPES, PATTERN_BEATS},
    validation::{ValidationError, validate_track},
};

#[derive(Clone, Copy)]
enum Modal {
    CommandPalette,
    EnvelopeEditor,
    ExportConfirm,
    FileBrowser,
    InstrumentEditor,
    PatternDeleteConfirm,
    QuitConfirm,
    TuningEditor,
}

pub struct App {
    input_path: PathBuf,
    keyboard_enhanced: bool,
    modal: Option<Modal>,
    terminal: DefaultTerminal,
    tracker_container: TrackerContainer,
    file_data: TrackerFile,
    config: GttConfig,
    beat_clipboard: Vec<BeatRow>,
    command_palette: CommandPalette,
    instrument_editor: InstrumentEditor,
    envelope_editor: EnvelopeEditor,
    tuning_editor: TuningEditor,
    file_browser: FileBrowser,
    quit_confirm: QuitConfirmModal,
    export_confirm: ExportConfirmModal,
    pattern_delete_confirm: PatternDeleteConfirmModal,
    player: Option<Player>,
    validation_errors: Vec<ValidationError>,
}

impl App {
    pub fn new(input_path: Option<PathBuf>) -> color_eyre::Result<Self> {
        let no_file = input_path.is_none();
        let input_path = input_path.unwrap_or_else(|| PathBuf::from("track.gtt"));
        let mut file_data = if input_path.exists() {
            TrackerFile::load(&input_path).map_err(|e| color_eyre::eyre::eyre!("{}", e))?
        } else {
            TrackerFile::empty()
        };
        let config: GttConfig = confy::load("gtt", None).unwrap_or_default();
        file_data.tuning.key_assignments = config_to_tuning_keys(&config.bindings.key_assignments);
        let terminal = ratatui::init();
        execute!(std::io::stdout(), event::EnableMouseCapture)?;
        let keyboard_enhanced = matches!(supports_keyboard_enhancement(), Ok(true))
            && execute!(
                std::io::stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )
            .is_ok();
        let tracker_container = TrackerContainer::init(&file_data);
        let player = Player::new(120);
        let mut file_browser = FileBrowser::new();
        let modal = if no_file {
            file_browser.open_file_explorer();
            Some(Modal::FileBrowser)
        } else {
            None
        };
        let mut app = Self {
            input_path,
            keyboard_enhanced,
            modal,
            terminal,
            tracker_container,
            file_data,
            config,
            beat_clipboard: Vec::new(),
            command_palette: CommandPalette::init(),
            instrument_editor: InstrumentEditor::init(),
            envelope_editor: EnvelopeEditor::init(),
            tuning_editor: TuningEditor::init(),
            file_browser,
            quit_confirm: QuitConfirmModal::init(),
            export_confirm: ExportConfirmModal::init(),
            pattern_delete_confirm: PatternDeleteConfirmModal::init(),
            player,
            validation_errors: Vec::new(),
        };
        app.revalidate();
        Ok(app)
    }

    fn write_file(&mut self) -> std::io::Result<()> {
        self.file_data.save(&self.input_path)
    }

    fn export_dir(&self) -> PathBuf {
        let stem = self
            .input_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "tracker".to_string());
        let parent = self
            .input_path
            .parent()
            .unwrap_or(std::path::Path::new("."));
        parent.join(format!("{}-export", stem))
    }

    fn run_export(&self, export_dir: &Path) -> std::io::Result<()> {
        let stem = self
            .input_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "track".to_string());
        export::export_all(&self.file_data, &stem, export_dir)
    }

    fn open_modal(&mut self, modal: Modal) -> bool {
        if self.modal.is_some() {
            return false;
        }
        self.modal = Some(modal);
        true
    }

    fn go_to_pattern(&mut self, offset: isize) {
        let idx = self.tracker_container.pattern_idx() as isize;
        let last = (self.file_data.patterns.len().max(1) - 1) as isize;
        let target = idx.saturating_add(offset).clamp(0, last);
        if target != idx {
            self.tracker_container.set_pattern_idx(target as u8);
        }
    }

    pub fn run(mut self) -> std::io::Result<()> {
        let _ = poll_events();
        let result = self.event_loop();
        if self.keyboard_enhanced {
            execute!(std::io::stdout(), PopKeyboardEnhancementFlags).ok();
        }
        execute!(std::io::stdout(), event::DisableMouseCapture).ok();
        result
    }

    fn event_loop(&mut self) -> std::io::Result<()> {
        loop {
            sleep(Duration::from_millis(16));

            if let Some(player) = &self.player
                && player.is_playing()
            {
                let row = player.current_row() as u8;
                self.tracker_container.set_current_row(row);
                let pattern_idx = player.current_pattern() as u8;
                if pattern_idx != self.tracker_container.pattern_idx() {
                    self.tracker_container.set_pattern_idx(pattern_idx);
                }
            }

            let events = poll_events();

            let file = &mut self.file_data;
            let actions = match self.modal {
                Some(Modal::CommandPalette) => self.command_palette.update(events, file),
                Some(Modal::EnvelopeEditor) => self.envelope_editor.update(events, file),
                Some(Modal::ExportConfirm) => self.export_confirm.update(events, file),
                Some(Modal::FileBrowser) => self.file_browser.update(events, file),
                Some(Modal::InstrumentEditor) => self.instrument_editor.update(events, file),
                Some(Modal::PatternDeleteConfirm) => {
                    self.pattern_delete_confirm.update(events, file)
                }
                Some(Modal::QuitConfirm) => self.quit_confirm.update(events, file),
                Some(Modal::TuningEditor) => self.tuning_editor.update(events, file),
                None => {
                    let mut hotkey_actions = Vec::new();
                    let mut tc_events = Vec::with_capacity(events.len());
                    let mut events = events.into_iter();
                    for e in events.by_ref() {
                        let Event::Key(key) = &e else {
                            tc_events.push(e);
                            continue;
                        };
                        if is_palette_key(key) && !self.tracker_container.is_editing_field() {
                            self.modal = Some(Modal::CommandPalette);
                            break;
                        } else if let Some(action) = hotkey_action(key)
                            && self.tracker_container.accepts_hotkeys()
                        {
                            hotkey_actions.push(action);
                        } else {
                            tc_events.push(e);
                        }
                    }
                    let mut actions = self.tracker_container.update(tc_events, file);
                    actions.extend(hotkey_actions);
                    if self.modal.is_some() {
                        actions.extend(self.command_palette.update(events.collect(), file));
                    }
                    actions
                }
            };

            if self.handle_actions(actions)? {
                return Ok(());
            }

            let modal = self.modal;
            let tc = &mut self.tracker_container;
            let cp = &mut self.command_palette;
            let ie = &mut self.instrument_editor;
            let ee = &mut self.envelope_editor;
            let te = &mut self.tuning_editor;
            let fb = &mut self.file_browser;
            let qc = &mut self.quit_confirm;
            let ec = &mut self.export_confirm;
            let pdc = &mut self.pattern_delete_confirm;
            let file = &self.file_data;
            self.terminal.draw(|f| {
                let area = f.area();
                tc.render(f, area, file);
                match modal {
                    Some(Modal::CommandPalette) => cp.render(
                        f,
                        Rect {
                            y: area.bottom().saturating_sub(1),
                            height: 1,
                            ..area
                        },
                        file,
                    ),
                    Some(Modal::EnvelopeEditor) => ee.render(f, area, file),
                    Some(Modal::ExportConfirm) => ec.render(f, area, file),
                    Some(Modal::FileBrowser) => fb.render(f, area, file),
                    Some(Modal::InstrumentEditor) => ie.render(f, area, file),
                    Some(Modal::PatternDeleteConfirm) => pdc.render(f, area, file),
                    Some(Modal::QuitConfirm) => qc.render(f, area, file),
                    Some(Modal::TuningEditor) => te.render(f, area, file),
                    None => {}
                }
            })?;
        }
    }

    fn handle_actions(&mut self, actions: Vec<ComponentAction>) -> std::io::Result<bool> {
        for action in actions {
            let edits_track = edits_track(&action);
            match action {
                ComponentAction::CloseModal => self.modal = None,
                ComponentAction::ConfirmExport(path) => {
                    let _ = std::fs::remove_dir_all(&path);
                    match self.run_export(&path) {
                        Ok(()) => {
                            self.modal = None;
                            self.tracker_container.mark_export_success();
                        }
                        Err(_) => {
                            self.export_confirm.set_error();
                        }
                    }
                }
                ComponentAction::CreateNewFile(path) => {
                    self.file_data = TrackerFile::empty();
                    self.file_data.tuning.key_assignments =
                        config_to_tuning_keys(&self.config.bindings.key_assignments);
                    self.input_path = path;
                    self.tracker_container = TrackerContainer::init(&self.file_data);
                    if let Some(player) = &mut self.player {
                        player.pause();
                    }
                }
                ComponentAction::DeleteBeats(count) => {
                    if let Some(rows) =
                        self.tracker_container.delete_beats(&mut self.file_data, count)
                    {
                        self.beat_clipboard = rows;
                    }
                }
                ComponentAction::Export => {
                    let export_dir = self.export_dir();
                    if export_dir.exists() {
                        if self.open_modal(Modal::ExportConfirm) {
                            self.export_confirm.open(export_dir);
                        }
                    } else {
                        let export_dir = export_dir.clone();
                        if self.run_export(&export_dir).is_err() {
                            if self.open_modal(Modal::ExportConfirm) {
                                self.export_confirm.open_error(export_dir);
                            }
                        } else {
                            self.tracker_container.mark_export_success();
                        }
                    }
                }
                ComponentAction::InstrumentSaved(idx, waveform) => {
                    self.file_data.instruments[idx].waveform = waveform.to_vec();
                }
                ComponentAction::EnvelopeSaved(idx, steps) => {
                    self.file_data.envelopes[idx].steps = steps;
                }
                ComponentAction::MuteChannel => {
                    if let Some(ch) = self.tracker_container.active_channel() {
                        self.tracker_container.set_channel_muted(ch, true);
                        if let Some(player) = &self.player {
                            player.set_channel_muted(ch, true);
                        }
                    }
                }
                ComponentAction::OpenFile(path) => match TrackerFile::load(&path) {
                    Ok(mut file_data) => {
                        file_data.tuning.key_assignments =
                            config_to_tuning_keys(&self.config.bindings.key_assignments);
                        self.file_data = file_data;
                        self.input_path = path;
                        self.tracker_container = TrackerContainer::init(&self.file_data);
                        if let Some(player) = &mut self.player {
                            player.pause();
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to load file: {}", e);
                    }
                },
                ComponentAction::OpenFileBrowser => {
                    if self.open_modal(Modal::FileBrowser)
                        && self.file_browser.file_explorer.is_none()
                    {
                        self.file_browser.open_file_explorer();
                    }
                }
                ComponentAction::OpenInstrumentEditor(idx) => {
                    if self.open_modal(Modal::InstrumentEditor) {
                        let name = &self.file_data.instruments[idx].name;
                        let waveform = self.file_data.instrument_waveform(idx);
                        self.instrument_editor.open(idx, name, &waveform);
                    }
                }
                ComponentAction::OpenEnvelopeEditor(idx) => {
                    if self.open_modal(Modal::EnvelopeEditor) {
                        let name = self.file_data.envelope_names()[idx].clone();
                        let steps = self.file_data.envelope_steps(idx);
                        self.envelope_editor.open(idx, &name, &steps);
                    }
                }
                ComponentAction::OpenPatternDeleteConfirm => {
                    if self.file_data.patterns.len() <= 1 {
                        let idx = self.tracker_container.pattern_idx();
                        *self.file_data.current_pattern_mut(idx) = crate::tracker::empty_pattern();
                        self.file_data.set_beats_for(idx, PATTERN_BEATS as u8);
                    } else if self.open_modal(Modal::PatternDeleteConfirm) {
                        self.pattern_delete_confirm.open();
                    }
                }
                ComponentAction::OpenQuitConfirm => {
                    if self.open_modal(Modal::QuitConfirm) {
                        self.quit_confirm.open();
                    }
                }
                ComponentAction::OpenTuningEditor => {
                    if self.open_modal(Modal::TuningEditor) {
                        self.tuning_editor.open(&self.file_data.tuning);
                    }
                }
                ComponentAction::PasteAbove | ComponentAction::PasteBelow => {
                    self.tracker_container.paste_beats(
                        &mut self.file_data,
                        &self.beat_clipboard,
                        matches!(action, ComponentAction::PasteBelow),
                    );
                }
                ComponentAction::PatternCopy => {
                    if self.file_data.patterns.len() >= MAX_PATTERNS {
                        continue;
                    }
                    let idx = self.tracker_container.pattern_idx() as usize;
                    let copy = self.file_data.current_pattern(idx as u8).clone();
                    let beats = self.file_data.beats_for(idx as u8);
                    let insert_at = (idx + 1).min(self.file_data.patterns.len());
                    self.file_data.patterns.insert(insert_at, copy);
                    self.file_data.pattern_beats.insert(insert_at, beats);
                    self.tracker_container.set_pattern_idx(insert_at as u8);
                }
                ComponentAction::PatternDelete => {
                    let idx = self.tracker_container.pattern_idx() as usize;
                    if self.file_data.patterns.len() <= 1 {
                        self.file_data.patterns[0] = crate::tracker::empty_pattern();
                        self.file_data.pattern_beats[0] = PATTERN_BEATS as u8;
                    } else if idx < self.file_data.patterns.len() {
                        self.file_data.patterns.remove(idx);
                        if idx < self.file_data.pattern_beats.len() {
                            self.file_data.pattern_beats.remove(idx);
                        }
                        let new_last = (self.file_data.patterns.len() - 1) as u8;
                        let new_idx = idx.min(new_last as usize) as u8;
                        self.tracker_container.set_pattern_idx(new_idx);
                    }
                }
                ComponentAction::PatternEdited => {}
                ComponentAction::PatternGoTo(offset) => self.go_to_pattern(offset),
                ComponentAction::PatternNew => {
                    if self.file_data.patterns.len() >= MAX_PATTERNS {
                        continue;
                    }
                    let idx = self.tracker_container.pattern_idx() as usize;
                    let insert_at = (idx + 1).min(self.file_data.patterns.len());
                    self.file_data
                        .patterns
                        .insert(insert_at, crate::tracker::empty_pattern());
                    self.file_data
                        .pattern_beats
                        .insert(insert_at, PATTERN_BEATS as u8);
                    self.tracker_container.set_pattern_idx(insert_at as u8);
                }
                ComponentAction::PatternNext => self.go_to_pattern(1),
                ComponentAction::PatternPrev => self.go_to_pattern(-1),
                ComponentAction::PatternZap => {
                    let idx = self.tracker_container.pattern_idx();
                    *self.file_data.current_pattern_mut(idx) = crate::tracker::empty_pattern();
                    self.file_data.set_beats_for(idx, PATTERN_BEATS as u8);
                }
                ComponentAction::Play => {
                    if let Some(player) = &self.player {
                        if player.is_playing() {
                            player.pause();
                            self.tracker_container.set_playing(false);
                        } else if self.validation_errors.is_empty() {
                            let row = self.tracker_container.current_row() as usize;
                            let pattern_idx = self.tracker_container.pattern_idx() as usize;
                            let patterns = self.file_data.patterns.clone();
                            let beats_list = self.file_data.pattern_beats.clone();
                            player.update_patterns(patterns, beats_list);
                            for i in 0..NUM_INSTRUMENTS {
                                player.update_waveform(i, self.file_data.instrument_waveform(i));
                            }
                            player.update_tuning_notes(self.file_data.tuning.notes.clone());
                            player.update_envelopes(
                                (0..NUM_ENVELOPES)
                                    .map(|i| self.file_data.envelope_steps(i))
                                    .collect(),
                            );
                            player.set_bpm(self.file_data.bpm);
                            player.set_fx_speed(self.file_data.fx_speed);
                            player.play(pattern_idx, row);
                            self.tracker_container.set_playing(true);
                        }
                    }
                }
                ComponentAction::Quit => return Ok(true),
                ComponentAction::RequestFocus => {}
                ComponentAction::SaveAndQuit => {
                    self.write_file()?;
                    return Ok(true);
                }
                ComponentAction::SaveFile => {
                    self.write_file()?;
                }
                ComponentAction::ToggleMuteChannel => {
                    if let Some(ch) = self.tracker_container.active_channel() {
                        let muted = !self.tracker_container.is_channel_muted(ch);
                        self.tracker_container.set_channel_muted(ch, muted);
                        if let Some(player) = &self.player {
                            player.set_channel_muted(ch, muted);
                        }
                    }
                }
                ComponentAction::TuningSaved(tuning) => {
                    self.file_data.tuning = tuning.clone();
                    self.tracker_container.apply_tuning(&tuning);
                    self.config.bindings.key_assignments =
                        build_key_assignments(&tuning.key_assignments);
                    let _ = confy::store("gtt", None, &self.config);
                }
                ComponentAction::UnmuteChannel => {
                    if let Some(ch) = self.tracker_container.active_channel() {
                        self.tracker_container.set_channel_muted(ch, false);
                        if let Some(player) = &self.player {
                            player.set_channel_muted(ch, false);
                        }
                    }
                }
                ComponentAction::SoloChannel => {
                    if let Some(ch) = self.tracker_container.active_channel() {
                        self.tracker_container.solo_channel(ch);
                        if let Some(player) = &self.player {
                            for c in 0..self.tracker_container.channel_count() {
                                player.set_channel_muted(
                                    c,
                                    self.tracker_container.is_channel_muted(c),
                                );
                            }
                        }
                    }
                }
                ComponentAction::YankBeats(count) => {
                    if let Some(rows) = self.tracker_container.yank_beats(&self.file_data, count) {
                        self.beat_clipboard = rows;
                    }
                }
            }
            if edits_track {
                self.revalidate();
            }
        }
        Ok(false)
    }

    fn revalidate(&mut self) {
        self.validation_errors = validate_track(&self.file_data);
        self.tracker_container
            .set_validation_errors(&self.validation_errors);
    }
}

fn edits_track(action: &ComponentAction) -> bool {
    matches!(
        action,
        ComponentAction::CreateNewFile(_)
            | ComponentAction::DeleteBeats(_)
            | ComponentAction::OpenFile(_)
            | ComponentAction::OpenPatternDeleteConfirm
            | ComponentAction::PasteAbove
            | ComponentAction::PasteBelow
            | ComponentAction::PatternCopy
            | ComponentAction::PatternDelete
            | ComponentAction::PatternEdited
            | ComponentAction::PatternNew
            | ComponentAction::PatternZap
    )
}

fn hotkey_action(key: &KeyEvent) -> Option<ComponentAction> {
    if key.kind != KeyEventKind::Press
        || key.modifiers.difference(KeyModifiers::SHIFT) != KeyModifiers::CONTROL
    {
        return None;
    }
    let c = match key.code {
        KeyCode::Char(c) => c,
        KeyCode::PageDown if key.modifiers == KeyModifiers::CONTROL => {
            return Some(ComponentAction::PatternNext);
        }
        KeyCode::PageUp if key.modifiers == KeyModifiers::CONTROL => {
            return Some(ComponentAction::PatternPrev);
        }
        _ => return None,
    };
    match c.to_ascii_lowercase() {
        'c' | 'y' => Some(ComponentAction::YankBeats(1)),
        'd' | 'x' => Some(ComponentAction::DeleteBeats(1)),
        'm' => Some(ComponentAction::ToggleMuteChannel),
        'p' if key.modifiers.contains(KeyModifiers::SHIFT) => Some(ComponentAction::PasteAbove),
        'p' => Some(ComponentAction::PasteBelow),
        's' => Some(ComponentAction::SoloChannel),
        _ => None,
    }
}

fn is_palette_key(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char(':')
        && key.kind == KeyEventKind::Press
        && (key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT)
}

fn poll_events() -> Vec<Event> {
    let mut events = vec![];
    while let Ok(true) = event::poll(Duration::from_millis(0)) {
        if let Ok(e) = event::read() {
            events.push(e);
        }
    }
    events
}
