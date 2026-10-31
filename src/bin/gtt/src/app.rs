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
    file::{NUM_INSTRUMENTS, TrackerFile},
    player::Player,
    tracker::{BeatRow, NUM_ENVELOPES},
};

pub struct App {
    input_path: PathBuf,
    keyboard_enhanced: bool,
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
        if no_file {
            file_browser.visible = true;
            file_browser.open_file_explorer();
        }
        Ok(Self {
            input_path,
            keyboard_enhanced,
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
        })
    }

    fn write_file(&mut self) -> std::io::Result<()> {
        let names = self.tracker_container.get_instrument_names();
        for (i, name) in names.iter().enumerate() {
            self.file_data.instruments[i].name = name.clone();
        }
        let envelope_names = self.tracker_container.get_envelope_names();
        for (i, name) in envelope_names.iter().enumerate() {
            self.file_data.envelopes[i].name = name.clone();
        }
        self.file_data.bpm = self.tracker_container.get_bpm();
        self.file_data.fx_speed = self.tracker_container.get_fx_speed();
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
        let bpm = self.tracker_container.get_bpm();
        let fx_speed = self.tracker_container.get_fx_speed();
        let stem = self
            .input_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "track".to_string());
        export::export_all(&self.file_data, bpm, fx_speed, &stem, export_dir)
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

            let actions = if self.quit_confirm.visible {
                self.quit_confirm.update(events, &mut self.file_data)
            } else if self.export_confirm.visible {
                self.export_confirm.update(events, &mut self.file_data)
            } else if self.pattern_delete_confirm.visible {
                self.pattern_delete_confirm.update(events, &mut self.file_data)
            } else if self.instrument_editor.visible {
                self.instrument_editor.update(events, &mut self.file_data)
            } else if self.envelope_editor.visible {
                self.envelope_editor.update(events, &mut self.file_data)
            } else if self.tuning_editor.visible {
                self.tuning_editor.update(events, &mut self.file_data)
            } else if self.file_browser.visible {
                self.file_browser.update(events, &mut self.file_data)
            } else if self.command_palette.visible {
                self.command_palette.update(events, &mut self.file_data)
            } else {
                let mut hotkey_actions = Vec::new();
                let mut tc_events = Vec::with_capacity(events.len());
                for e in events {
                    let Event::Key(key) = &e else {
                        tc_events.push(e);
                        continue;
                    };
                    if is_palette_key(key) && !self.tracker_container.is_editing_field() {
                        self.command_palette.visible = true;
                    } else if let Some(action) = hotkey_action(key)
                        && self.tracker_container.accepts_hotkeys()
                    {
                        hotkey_actions.push(action);
                    } else {
                        tc_events.push(e);
                    }
                }
                let mut actions = self
                    .tracker_container
                    .update(tc_events, &mut self.file_data);
                actions.extend(hotkey_actions);
                actions
            };

            if self.handle_actions(actions)? {
                return Ok(());
            }

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
                if cp.visible {
                    cp.render(
                        f,
                        Rect {
                            y: area.bottom().saturating_sub(1),
                            height: 1,
                            ..area
                        },
                        file,
                    );
                }
                if ie.visible {
                    ie.render(f, area, file);
                }
                if ee.visible {
                    ee.render(f, area, file);
                }
                if te.visible {
                    te.render(f, area, file);
                }
                if fb.visible {
                    fb.render(f, area, file);
                }
                if qc.visible {
                    qc.render(f, area, file);
                }
                if ec.visible {
                    ec.render(f, area, file);
                }
                if pdc.visible {
                    pdc.render(f, area, file);
                }
            })?;
        }
    }

    fn handle_actions(&mut self, actions: Vec<ComponentAction>) -> std::io::Result<bool> {
        for action in actions {
            match action {
                ComponentAction::ConfirmExport(path) => {
                    let _ = std::fs::remove_dir_all(&path);
                    match self.run_export(&path) {
                        Ok(()) => {
                            self.export_confirm.visible = false;
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
                        self.export_confirm.open(export_dir);
                    } else {
                        let export_dir = export_dir.clone();
                        if self.run_export(&export_dir).is_err() {
                            self.export_confirm.open_error(export_dir);
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
                    self.file_browser.visible = true;
                    if self.file_browser.file_explorer.is_none() {
                        self.file_browser.open_file_explorer();
                    }
                }
                ComponentAction::OpenInstrumentEditor(idx) => {
                    let name = self.tracker_container.get_instrument_names()[idx].clone();
                    let waveform = self.file_data.instrument_waveform(idx);
                    self.instrument_editor.open(idx, &name, &waveform);
                }
                ComponentAction::OpenEnvelopeEditor(idx) => {
                    let name = self.tracker_container.get_envelope_names()[idx].clone();
                    let steps = self.file_data.envelope_steps(idx);
                    self.envelope_editor.open(idx, &name, &steps);
                }
                ComponentAction::OpenPatternDeleteConfirm => {
                    if self.file_data.patterns.len() <= 1 {
                        let idx = self.tracker_container.pattern_idx();
                        *self.file_data.current_pattern_mut(idx) = crate::tracker::empty_pattern();
                        self.file_data.set_beats_for(idx, 64);
                    } else {
                        self.pattern_delete_confirm.open();
                    }
                }
                ComponentAction::OpenQuitConfirm => {
                    self.quit_confirm.open();
                }
                ComponentAction::OpenTuningEditor => {
                    self.tuning_editor.open(&self.file_data.tuning);
                }
                ComponentAction::PasteAbove | ComponentAction::PasteBelow => {
                    self.tracker_container.paste_beats(
                        &mut self.file_data,
                        &self.beat_clipboard,
                        matches!(action, ComponentAction::PasteBelow),
                    );
                }
                ComponentAction::PatternCopy => {
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
                        self.file_data.pattern_beats[0] = 64;
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
                ComponentAction::PatternGoTo(offset) => self.go_to_pattern(offset),
                ComponentAction::PatternNew => {
                    let idx = self.tracker_container.pattern_idx() as usize;
                    let insert_at = (idx + 1).min(self.file_data.patterns.len());
                    self.file_data
                        .patterns
                        .insert(insert_at, crate::tracker::empty_pattern());
                    self.file_data.pattern_beats.insert(insert_at, 64);
                    self.tracker_container.set_pattern_idx(insert_at as u8);
                }
                ComponentAction::PatternNext => self.go_to_pattern(1),
                ComponentAction::PatternPrev => self.go_to_pattern(-1),
                ComponentAction::PatternZap => {
                    let idx = self.tracker_container.pattern_idx();
                    *self.file_data.current_pattern_mut(idx) = crate::tracker::empty_pattern();
                    self.file_data.set_beats_for(idx, 64);
                }
                ComponentAction::Play => {
                    if let Some(player) = &self.player {
                        if player.is_playing() {
                            player.pause();
                            self.tracker_container.set_playing(false);
                        } else {
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
                            player.set_bpm(self.tracker_container.get_bpm());
                            player.set_fx_speed(self.tracker_container.get_fx_speed());
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
        }
        Ok(false)
    }
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
