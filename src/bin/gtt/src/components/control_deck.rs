use std::time::{Duration, Instant};

use rat_widget::button::{Button, ButtonState};
use rat_widget::text::HasScreenCursor;
use rat_widget::text_input::{TextInput, TextInputState, handle_events};
use ratatui::{
    Frame,
    crossterm::event::{
        Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
    },
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Paragraph},
};

use crate::{
    action::ComponentAction,
    component::Component,
    file::{NUM_INSTRUMENTS, TrackerFile},
    scheme::SCHEME,
    tracker::{MAX_FX_SPEED, MAX_TEMPO, NUM_ENVELOPES, PATTERN_BEATS, PATTERN_TABLE_WIDTH},
    validation::ValidationError,
};

const BTN_W: u16 = 3;

struct SlotStyles {
    default_style: Style,
    selected_style: Style,
    btn_base: Style,
    btn_focus: Style,
}

fn render_named_slots(
    frame: &mut Frame,
    entries: &mut [NamedSlotEntry],
    rows: &[Rect],
    selected: Option<(usize, Col)>,
    editing: bool,
    styles: &SlotStyles,
) {
    for (idx, entry) in entries.iter_mut().enumerate() {
        let Some(&cell_area) = rows.get(idx) else {
            break;
        };
        let open_focused = selected == Some((idx, Col::Open));
        let input_focused = selected == Some((idx, Col::Input));

        let [btn_area, _, input_area] = Layout::horizontal([
            Constraint::Length(BTN_W),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(cell_area);

        let open_line = Line::from("[⚙]").style(if open_focused {
            styles.btn_focus
        } else {
            styles.btn_base
        });
        entry.open_button.focus.set(open_focused);
        frame.render_stateful_widget(
            Button::new(open_line)
                .style(styles.btn_base)
                .focus_style(styles.btn_focus),
            btn_area,
            &mut entry.open_button,
        );

        entry.name_input.focus.set(input_focused && editing);
        frame.render_stateful_widget(
            TextInput::new().style(if input_focused {
                styles.selected_style
            } else {
                styles.default_style
            }),
            input_area,
            &mut entry.name_input,
        );

        if input_focused
            && editing
            && let Some((cx, cy)) = entry.name_input.screen_cursor()
        {
            frame.set_cursor_position((cx, cy));
        }
    }
}

struct NamedSlotEntry {
    open_button: ButtonState,
    name_input: TextInputState,
}

impl NamedSlotEntry {
    fn new() -> Self {
        Self {
            open_button: ButtonState::new(),
            name_input: TextInputState::new(),
        }
    }
}

fn show_value(input: &mut TextInputState, value: &str) {
    if input.value::<String>() != value {
        input.set_value(value);
    }
}

fn parse_setting(row: Row, raw: &str) -> u16 {
    let (min, max) = row.range();
    u16::from_str_radix(raw, 16).map_or(min, |v| v.clamp(min, max))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Bpm,
    FxSpeed,
    Beats,
    Trans,
    ActionPlay,
    ActionNewOpen,
    Instrument(usize),
    Envelope(usize),
    ActionTuning,
    ActionQuit,
    ActionSave,
    ActionExport,
    PatternPrev,
    PatternNext,
    PatternNew,
    PatternCopy,
    PatternDelete,
    PatternZap,
}

impl Row {
    fn is_setting(self) -> bool {
        matches!(self, Row::Bpm | Row::FxSpeed | Row::Beats | Row::Trans)
    }

    fn is_action(self) -> bool {
        matches!(
            self,
            Row::ActionPlay
                | Row::ActionNewOpen
                | Row::ActionTuning
                | Row::ActionQuit
                | Row::ActionSave
                | Row::ActionExport
                | Row::PatternPrev
                | Row::PatternNext
                | Row::PatternNew
                | Row::PatternCopy
                | Row::PatternDelete
                | Row::PatternZap
        )
    }

    fn label(self) -> &'static str {
        match self {
            Row::Bpm => "BPM:     ",
            Row::FxSpeed => "FxSpeed: ",
            Row::Beats => "Beats:   ",
            Row::Trans => "Trans:   ",
            Row::Instrument(_)
            | Row::Envelope(_)
            | Row::ActionPlay
            | Row::ActionNewOpen
            | Row::ActionTuning
            | Row::ActionQuit
            | Row::ActionSave
            | Row::ActionExport
            | Row::PatternPrev
            | Row::PatternNext
            | Row::PatternNew
            | Row::PatternCopy
            | Row::PatternDelete
            | Row::PatternZap => "",
        }
    }

    fn range(self) -> (u16, u16) {
        match self {
            Row::Bpm => (1, MAX_TEMPO),
            Row::FxSpeed => (1, MAX_FX_SPEED as u16),
            Row::Beats => (1, PATTERN_BEATS as u16),
            Row::Trans
            | Row::Instrument(_)
            | Row::Envelope(_)
            | Row::ActionPlay
            | Row::ActionNewOpen
            | Row::ActionTuning
            | Row::ActionQuit
            | Row::ActionSave
            | Row::ActionExport
            | Row::PatternPrev
            | Row::PatternNext
            | Row::PatternNew
            | Row::PatternCopy
            | Row::PatternDelete
            | Row::PatternZap => (0, 0),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Input,
    Plus,
    Minus,
    Open,
}

impl Col {
    fn next(self, row: Row) -> Self {
        if row.is_setting() {
            match self {
                Col::Input => Col::Plus,
                Col::Plus => Col::Minus,
                _ => Col::Input,
            }
        } else if row.is_action() {
            Col::Open
        } else {
            match self {
                Col::Open => Col::Input,
                _ => Col::Open,
            }
        }
    }

    fn prev(self, row: Row) -> Self {
        if row.is_setting() {
            match self {
                Col::Minus => Col::Plus,
                Col::Plus => Col::Input,
                _ => Col::Minus,
            }
        } else if row.is_action() {
            Col::Open
        } else {
            match self {
                Col::Input => Col::Open,
                _ => Col::Input,
            }
        }
    }
}

fn default_col(row: Row) -> Col {
    match row {
        Row::Bpm | Row::FxSpeed | Row::Beats | Row::Trans => Col::Input,
        Row::Instrument(_)
        | Row::Envelope(_)
        | Row::ActionPlay
        | Row::ActionNewOpen
        | Row::ActionTuning
        | Row::ActionQuit
        | Row::ActionSave
        | Row::ActionExport
        | Row::PatternPrev
        | Row::PatternNext
        | Row::PatternNew
        | Row::PatternCopy
        | Row::PatternDelete
        | Row::PatternZap => Col::Open,
    }
}

pub struct ControlDeck {
    action_export_btn: ButtonState,
    action_new_open_btn: ButtonState,
    action_play_btn: ButtonState,
    action_quit_btn: ButtonState,
    action_save_btn: ButtonState,
    action_tuning_btn: ButtonState,
    bpm_input: TextInputState,
    bpm_minus: ButtonState,
    bpm_plus: ButtonState,
    fx_speed_input: TextInputState,
    fx_speed_minus: ButtonState,
    fx_speed_plus: ButtonState,
    pub editing: bool,
    instruments: [NamedSlotEntry; NUM_INSTRUMENTS],
    envelopes: [NamedSlotEntry; NUM_ENVELOPES],
    pub playing: bool,
    rows_input: TextInputState,
    rows_minus: ButtonState,
    rows_plus: ButtonState,
    save_feedback_until: Option<Instant>,
    export_feedback_until: Option<Instant>,
    selected_col: Col,
    selected_row: Row,
    trans_input: TextInputState,
    trans_minus: ButtonState,
    trans_plus: ButtonState,
    trans_snapshot: String,
    trans_assigned_indices: Vec<usize>,
    trans_note_count: usize,
    trans_scale_size: usize,
    pattern_prev_btn: ButtonState,
    pattern_next_btn: ButtonState,
    pattern_new_btn: ButtonState,
    pattern_copy_btn: ButtonState,
    pattern_delete_btn: ButtonState,
    pattern_zap_btn: ButtonState,
    pattern_idx: u8,
    pattern_count: usize,
    pattern_resized: bool,
    validation_errors: Vec<ValidationError>,
}

impl ControlDeck {
    pub fn init() -> Self {
        let mut trans_input = TextInputState::new();
        trans_input.set_value("0");
        Self {
            selected_row: Row::Bpm,
            selected_col: Col::Input,
            editing: false,
            bpm_input: TextInputState::new(),
            bpm_plus: ButtonState::new(),
            bpm_minus: ButtonState::new(),
            fx_speed_input: TextInputState::new(),
            fx_speed_plus: ButtonState::new(),
            fx_speed_minus: ButtonState::new(),
            rows_input: TextInputState::new(),
            rows_plus: ButtonState::new(),
            rows_minus: ButtonState::new(),
            trans_input,
            trans_snapshot: "0".to_string(),
            trans_plus: ButtonState::new(),
            trans_minus: ButtonState::new(),
            trans_assigned_indices: Vec::new(),
            trans_note_count: 0,
            trans_scale_size: 0,
            instruments: std::array::from_fn(|_| NamedSlotEntry::new()),
            envelopes: std::array::from_fn(|_| NamedSlotEntry::new()),
            playing: false,
            action_tuning_btn: ButtonState::new(),
            action_new_open_btn: ButtonState::new(),
            action_play_btn: ButtonState::new(),
            action_quit_btn: ButtonState::new(),
            action_save_btn: ButtonState::new(),
            action_export_btn: ButtonState::new(),
            pattern_prev_btn: ButtonState::new(),
            pattern_next_btn: ButtonState::new(),
            pattern_new_btn: ButtonState::new(),
            pattern_copy_btn: ButtonState::new(),
            pattern_delete_btn: ButtonState::new(),
            pattern_zap_btn: ButtonState::new(),
            pattern_idx: 0,
            pattern_count: 1,
            pattern_resized: false,
            save_feedback_until: None,
            export_feedback_until: None,
            validation_errors: Vec::new(),
        }
    }

    pub fn blur_all(&mut self, file: &mut TrackerFile) -> Vec<ComponentAction> {
        if self.editing {
            self.confirm_editing(file);
        }
        self.bpm_input.focus.set(false);
        self.bpm_plus.focus.set(false);
        self.bpm_minus.focus.set(false);
        self.fx_speed_input.focus.set(false);
        self.fx_speed_plus.focus.set(false);
        self.fx_speed_minus.focus.set(false);
        self.rows_input.focus.set(false);
        self.rows_plus.focus.set(false);
        self.rows_minus.focus.set(false);
        self.trans_input.focus.set(false);
        self.trans_plus.focus.set(false);
        self.trans_minus.focus.set(false);
        for entry in self.instruments.iter_mut().chain(self.envelopes.iter_mut()) {
            entry.open_button.focus.set(false);
            entry.name_input.focus.set(false);
        }
        self.action_tuning_btn.focus.set(false);
        self.action_new_open_btn.focus.set(false);
        self.action_play_btn.focus.set(false);
        self.action_quit_btn.focus.set(false);
        self.action_save_btn.focus.set(false);
        self.action_export_btn.focus.set(false);
        self.pattern_prev_btn.focus.set(false);
        self.pattern_next_btn.focus.set(false);
        self.pattern_new_btn.focus.set(false);
        self.pattern_copy_btn.focus.set(false);
        self.pattern_delete_btn.focus.set(false);
        self.pattern_zap_btn.focus.set(false);
        self.take_edit_actions()
    }

    pub fn mark_export_success(&mut self) {
        self.export_feedback_until = Some(Instant::now() + Duration::from_secs(3));
    }

    pub fn set_pattern_info(&mut self, pattern_idx: u8, pattern_count: usize) {
        self.pattern_idx = pattern_idx;
        self.pattern_count = pattern_count.max(1);
    }

    pub fn set_validation_errors(&mut self, errors: &[ValidationError]) {
        self.validation_errors = errors.to_vec();
    }

    fn refresh_from_file(&mut self, file: &TrackerFile) {
        let editing = self.editing.then_some(self.selected_row);
        let beats = file.beats_for(self.pattern_idx);
        for (row, input, value) in [
            (Row::Bpm, &mut self.bpm_input, file.bpm),
            (Row::FxSpeed, &mut self.fx_speed_input, file.fx_speed as u16),
            (Row::Beats, &mut self.rows_input, beats as u16),
        ] {
            if editing != Some(row) {
                show_value(input, &format!("{:X}", value));
            }
        }
        for (i, entry) in self.instruments.iter_mut().enumerate() {
            if editing != Some(Row::Instrument(i)) {
                show_value(&mut entry.name_input, &file.instruments[i].name);
            }
        }
        for (i, entry) in self.envelopes.iter_mut().enumerate() {
            if editing != Some(Row::Envelope(i)) {
                let name = file.envelopes.get(i).map_or("", |e| e.name.as_str());
                show_value(&mut entry.name_input, name);
            }
        }
    }

    fn take_edit_actions(&mut self) -> Vec<ComponentAction> {
        if std::mem::take(&mut self.pattern_resized) {
            vec![ComponentAction::PatternResized]
        } else {
            Vec::new()
        }
    }

    pub fn get_transpose(&self) -> i32 {
        self.trans_input
            .value::<String>()
            .parse::<i32>()
            .unwrap_or(0)
    }

    pub fn set_tuning_context(
        &mut self,
        scale_size: usize,
        note_count: usize,
        assigned_indices: Vec<usize>,
    ) {
        self.trans_scale_size = scale_size;
        self.trans_note_count = note_count;
        self.trans_assigned_indices = assigned_indices;
        let current = self.get_transpose();
        if !self.is_transpose_valid(current) {
            self.trans_input.set_value("0");
            self.trans_snapshot = "0".to_string();
        }
    }

    fn is_transpose_valid(&self, t: i32) -> bool {
        if self.trans_scale_size == 0 {
            return t == 0;
        }
        let shift = t * self.trans_scale_size as i32;
        self.trans_assigned_indices.iter().any(|&idx| {
            let shifted = idx as i32 + shift;
            shifted >= 0 && shifted < self.trans_note_count as i32
        })
    }

    fn update_focus_states(&mut self) {
        let row = self.selected_row;
        let col = self.selected_col;
        let editing = self.editing;
        self.bpm_input
            .focus
            .set(row == Row::Bpm && col == Col::Input && editing);
        self.bpm_plus.focus.set(row == Row::Bpm && col == Col::Plus);
        self.bpm_minus
            .focus
            .set(row == Row::Bpm && col == Col::Minus);
        self.fx_speed_input
            .focus
            .set(row == Row::FxSpeed && col == Col::Input && editing);
        self.fx_speed_plus
            .focus
            .set(row == Row::FxSpeed && col == Col::Plus);
        self.fx_speed_minus
            .focus
            .set(row == Row::FxSpeed && col == Col::Minus);
        self.rows_input
            .focus
            .set(row == Row::Beats && col == Col::Input && editing);
        self.rows_plus
            .focus
            .set(row == Row::Beats && col == Col::Plus);
        self.rows_minus
            .focus
            .set(row == Row::Beats && col == Col::Minus);
        self.trans_input
            .focus
            .set(row == Row::Trans && col == Col::Input && editing);
        self.trans_plus
            .focus
            .set(row == Row::Trans && col == Col::Plus);
        self.trans_minus
            .focus
            .set(row == Row::Trans && col == Col::Minus);
        for i in 0..NUM_INSTRUMENTS {
            let is_row = row == Row::Instrument(i);
            self.instruments[i]
                .open_button
                .focus
                .set(is_row && col == Col::Open);
            self.instruments[i]
                .name_input
                .focus
                .set(is_row && col == Col::Input && editing);
        }
        for i in 0..NUM_ENVELOPES {
            let is_row = row == Row::Envelope(i);
            self.envelopes[i]
                .open_button
                .focus
                .set(is_row && col == Col::Open);
            self.envelopes[i]
                .name_input
                .focus
                .set(is_row && col == Col::Input && editing);
        }
        self.action_tuning_btn.focus.set(row == Row::ActionTuning);
        self.action_new_open_btn
            .focus
            .set(row == Row::ActionNewOpen);
        self.action_play_btn.focus.set(row == Row::ActionPlay);
        self.action_quit_btn.focus.set(row == Row::ActionQuit);
        self.action_save_btn.focus.set(row == Row::ActionSave);
        self.action_export_btn.focus.set(row == Row::ActionExport);
        self.pattern_prev_btn.focus.set(row == Row::PatternPrev);
        self.pattern_next_btn.focus.set(row == Row::PatternNext);
        self.pattern_new_btn.focus.set(row == Row::PatternNew);
        self.pattern_copy_btn.focus.set(row == Row::PatternCopy);
        self.pattern_delete_btn.focus.set(row == Row::PatternDelete);
        self.pattern_zap_btn.focus.set(row == Row::PatternZap);
    }

    fn current_input_mut(&mut self) -> &mut TextInputState {
        match self.selected_row {
            Row::Bpm => &mut self.bpm_input,
            Row::FxSpeed => &mut self.fx_speed_input,
            Row::Beats => &mut self.rows_input,
            Row::Trans => &mut self.trans_input,
            Row::Instrument(i) => &mut self.instruments[i].name_input,
            Row::Envelope(i) => &mut self.envelopes[i].name_input,
            Row::ActionPlay
            | Row::ActionNewOpen
            | Row::ActionTuning
            | Row::ActionQuit
            | Row::ActionSave
            | Row::ActionExport
            | Row::PatternPrev
            | Row::PatternNext
            | Row::PatternNew
            | Row::PatternCopy
            | Row::PatternDelete
            | Row::PatternZap => {
                unreachable!("action rows have no text input")
            }
        }
    }

    fn restore_snapshot(&mut self, file: &TrackerFile) {
        if self.selected_row == Row::Trans {
            let s = self.trans_snapshot.clone();
            self.trans_input.set_value(s);
        }
        self.editing = false;
        self.refresh_from_file(file);
    }

    fn start_editing(&mut self) {
        if self.selected_row.is_action() {
            return;
        }
        if self.selected_row == Row::Trans {
            self.trans_snapshot = self.trans_input.value::<String>();
        }
        self.editing = true;
    }

    fn confirm_editing(&mut self, file: &mut TrackerFile) {
        self.editing = false;
        match self.selected_row {
            row @ (Row::Bpm | Row::FxSpeed | Row::Beats) => {
                let raw = self.current_input_mut().value::<String>();
                self.set_setting_value(file, row, parse_setting(row, &raw));
            }
            Row::Trans => {
                let raw = self.trans_input.value::<String>();
                let parsed = raw.parse::<i32>().unwrap_or(0);
                let valid_val = if self.is_transpose_valid(parsed) {
                    parsed
                } else {
                    self.trans_snapshot.parse::<i32>().unwrap_or(0)
                };
                let s = valid_val.to_string();
                self.trans_input.set_value(s.clone());
                self.trans_snapshot = s;
            }
            Row::Instrument(i) => {
                file.instruments[i].name = self.instruments[i].name_input.value::<String>();
            }
            Row::Envelope(i) => {
                if let Some(envelope) = file.envelopes.get_mut(i) {
                    envelope.name = self.envelopes[i].name_input.value::<String>();
                }
            }
            Row::ActionPlay
            | Row::ActionNewOpen
            | Row::ActionTuning
            | Row::ActionQuit
            | Row::ActionSave
            | Row::ActionExport
            | Row::PatternPrev
            | Row::PatternNext
            | Row::PatternNew
            | Row::PatternCopy
            | Row::PatternDelete
            | Row::PatternZap => {}
        }
        self.refresh_from_file(file);
    }

    fn get_setting_value(&self, file: &TrackerFile, row: Row) -> u16 {
        match row {
            Row::Bpm => file.bpm,
            Row::FxSpeed => file.fx_speed as u16,
            Row::Beats => file.beats_for(self.pattern_idx) as u16,
            _ => row.range().0,
        }
    }

    fn set_setting_value(&mut self, file: &mut TrackerFile, row: Row, value: u16) {
        match row {
            Row::Bpm => file.bpm = value,
            Row::FxSpeed => file.fx_speed = value as u8,
            Row::Beats if file.beats_for(self.pattern_idx) != value as u8 => {
                file.set_beats_for(self.pattern_idx, value as u8);
                self.pattern_resized = true;
            }
            _ => {}
        }
    }

    fn increment(&mut self, file: &mut TrackerFile) {
        if self.selected_row == Row::Trans {
            let next = self.get_transpose() + 1;
            if self.is_transpose_valid(next) {
                let s = next.to_string();
                self.trans_input.set_value(s.clone());
                self.trans_snapshot = s;
            }
            return;
        }
        let row = self.selected_row;
        let (_, max) = row.range();
        let val = self.get_setting_value(file, row).saturating_add(1).min(max);
        self.set_setting_value(file, row, val);
        self.refresh_from_file(file);
    }

    fn decrement(&mut self, file: &mut TrackerFile) {
        if self.selected_row == Row::Trans {
            let next = self.get_transpose() - 1;
            if self.is_transpose_valid(next) {
                let s = next.to_string();
                self.trans_input.set_value(s.clone());
                self.trans_snapshot = s;
            }
            return;
        }
        let row = self.selected_row;
        let (min, _) = row.range();
        let val = self.get_setting_value(file, row).saturating_sub(1).max(min);
        self.set_setting_value(file, row, val);
        self.refresh_from_file(file);
    }

    fn handle_mouse_click(
        &mut self,
        pos: Position,
        file: &mut TrackerFile,
        actions: &mut Vec<ComponentAction>,
    ) -> bool {
        let bpm_plus_area = self.bpm_plus.area;
        let bpm_minus_area = self.bpm_minus.area;
        let fx_speed_plus_area = self.fx_speed_plus.area;
        let fx_speed_minus_area = self.fx_speed_minus.area;
        let rows_plus_area = self.rows_plus.area;
        let rows_minus_area = self.rows_minus.area;
        let bpm_input_area = self.bpm_input.area;
        let fx_speed_input_area = self.fx_speed_input.area;
        let rows_input_area = self.rows_input.area;
        let trans_plus_area = self.trans_plus.area;
        let trans_minus_area = self.trans_minus.area;
        let trans_input_area = self.trans_input.area;

        if bpm_plus_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::Bpm;
            self.selected_col = Col::Plus;
            self.increment(file);
            self.update_focus_states();
            return true;
        }
        if bpm_minus_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::Bpm;
            self.selected_col = Col::Minus;
            self.decrement(file);
            self.update_focus_states();
            return true;
        }
        if fx_speed_plus_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::FxSpeed;
            self.selected_col = Col::Plus;
            self.increment(file);
            self.update_focus_states();
            return true;
        }
        if fx_speed_minus_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::FxSpeed;
            self.selected_col = Col::Minus;
            self.decrement(file);
            self.update_focus_states();
            return true;
        }
        if rows_plus_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::Beats;
            self.selected_col = Col::Plus;
            self.increment(file);
            self.update_focus_states();
            return true;
        }
        if rows_minus_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::Beats;
            self.selected_col = Col::Minus;
            self.decrement(file);
            self.update_focus_states();
            return true;
        }
        if trans_plus_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::Trans;
            self.selected_col = Col::Plus;
            self.increment(file);
            self.update_focus_states();
            return true;
        }
        if trans_minus_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::Trans;
            self.selected_col = Col::Minus;
            self.decrement(file);
            self.update_focus_states();
            return true;
        }
        if bpm_input_area.contains(pos) {
            if self.editing && self.selected_row == Row::Bpm && self.selected_col == Col::Input {
                return true;
            }
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::Bpm;
            self.selected_col = Col::Input;
            self.start_editing();
            self.update_focus_states();
            return true;
        }
        if fx_speed_input_area.contains(pos) {
            if self.editing && self.selected_row == Row::FxSpeed && self.selected_col == Col::Input
            {
                return true;
            }
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::FxSpeed;
            self.selected_col = Col::Input;
            self.start_editing();
            self.update_focus_states();
            return true;
        }
        if rows_input_area.contains(pos) {
            if self.editing && self.selected_row == Row::Beats && self.selected_col == Col::Input {
                return true;
            }
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::Beats;
            self.selected_col = Col::Input;
            self.start_editing();
            self.update_focus_states();
            return true;
        }
        if trans_input_area.contains(pos) {
            if self.editing && self.selected_row == Row::Trans && self.selected_col == Col::Input {
                return true;
            }
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::Trans;
            self.selected_col = Col::Input;
            self.start_editing();
            self.update_focus_states();
            return true;
        }

        let play_area = self.action_play_btn.area;
        if play_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::ActionPlay;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::Play);
            self.update_focus_states();
            return true;
        }

        for i in 0..NUM_INSTRUMENTS {
            let open_area = self.instruments[i].open_button.area;
            let name_area = self.instruments[i].name_input.area;
            if open_area.contains(pos) {
                if self.editing {
                    self.confirm_editing(file);
                }
                self.selected_row = Row::Instrument(i);
                self.selected_col = Col::Open;
                actions.push(ComponentAction::OpenInstrumentEditor(i));
                self.update_focus_states();
                return true;
            }
            if name_area.contains(pos) {
                if self.editing
                    && self.selected_row == Row::Instrument(i)
                    && self.selected_col == Col::Input
                {
                    return true;
                }
                if self.editing {
                    self.confirm_editing(file);
                }
                self.selected_row = Row::Instrument(i);
                self.selected_col = Col::Input;
                self.start_editing();
                self.update_focus_states();
                return true;
            }
        }

        for i in 0..NUM_ENVELOPES {
            let open_area = self.envelopes[i].open_button.area;
            let name_area = self.envelopes[i].name_input.area;
            if open_area.contains(pos) {
                if self.editing {
                    self.confirm_editing(file);
                }
                self.selected_row = Row::Envelope(i);
                self.selected_col = Col::Open;
                actions.push(ComponentAction::OpenEnvelopeEditor(i));
                self.update_focus_states();
                return true;
            }
            if name_area.contains(pos) {
                if self.editing
                    && self.selected_row == Row::Envelope(i)
                    && self.selected_col == Col::Input
                {
                    return true;
                }
                if self.editing {
                    self.confirm_editing(file);
                }
                self.selected_row = Row::Envelope(i);
                self.selected_col = Col::Input;
                self.start_editing();
                self.update_focus_states();
                return true;
            }
        }

        let tuning_area = self.action_tuning_btn.area;
        let new_open_area = self.action_new_open_btn.area;
        let quit_area = self.action_quit_btn.area;
        let save_area = self.action_save_btn.area;
        let export_area = self.action_export_btn.area;

        if tuning_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::ActionTuning;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::OpenTuningEditor);
            self.update_focus_states();
            return true;
        }
        if new_open_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::ActionNewOpen;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::OpenFileBrowser);
            self.update_focus_states();
            return true;
        }
        if quit_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::ActionQuit;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::OpenQuitConfirm);
            self.update_focus_states();
            return true;
        }
        if save_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::ActionSave;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::SaveFile);
            self.save_feedback_until = Some(Instant::now() + Duration::from_secs(3));
            self.update_focus_states();
            return true;
        }
        if export_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::ActionExport;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::Export);
            self.update_focus_states();
            return true;
        }

        let pattern_prev_area = self.pattern_prev_btn.area;
        let pattern_next_area = self.pattern_next_btn.area;
        let pattern_new_area = self.pattern_new_btn.area;
        let pattern_copy_area = self.pattern_copy_btn.area;
        let pattern_delete_area = self.pattern_delete_btn.area;
        let pattern_zap_area = self.pattern_zap_btn.area;

        if pattern_prev_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::PatternPrev;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::PatternPrev);
            self.update_focus_states();
            return true;
        }
        if pattern_next_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::PatternNext;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::PatternNext);
            self.update_focus_states();
            return true;
        }
        if pattern_new_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::PatternNew;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::PatternNew);
            self.update_focus_states();
            return true;
        }
        if pattern_copy_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::PatternCopy;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::PatternCopy);
            self.update_focus_states();
            return true;
        }
        if pattern_delete_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::PatternDelete;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::OpenPatternDeleteConfirm);
            self.update_focus_states();
            return true;
        }
        if pattern_zap_area.contains(pos) {
            if self.editing {
                self.confirm_editing(file);
            }
            self.selected_row = Row::PatternZap;
            self.selected_col = Col::Open;
            actions.push(ComponentAction::PatternZap);
            self.update_focus_states();
            return true;
        }

        false
    }
}

impl Component for ControlDeck {
    fn update(&mut self, events: Vec<Event>, file: &mut TrackerFile) -> Vec<ComponentAction> {
        self.refresh_from_file(file);
        let mut actions = Vec::new();

        for event in &events {
            match event {
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column,
                    row,
                    ..
                }) => {
                    if self.handle_mouse_click(
                        Position {
                            x: *column,
                            y: *row,
                        },
                        file,
                        &mut actions,
                    ) {
                        actions.push(ComponentAction::RequestFocus);
                    }
                }
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if self.editing {
                        match code {
                            KeyCode::Esc => {
                                self.restore_snapshot(file);
                                self.update_focus_states();
                            }
                            KeyCode::Enter => {
                                self.confirm_editing(file);
                                self.update_focus_states();
                            }
                            KeyCode::Char(c)
                                if matches!(self.selected_row, Row::Bpm | Row::FxSpeed | Row::Beats)
                                    && !c.is_ascii_hexdigit() => {}
                            KeyCode::Char(c)
                                if self.selected_row == Row::Trans
                                    && !c.is_ascii_digit()
                                    && *c != '-' => {}
                            _ => {
                                let state = self.current_input_mut();
                                state.focus.set(true);
                                let _ = handle_events(state, true, event);
                            }
                        }
                    } else {
                        match code {
                            KeyCode::Up => {
                                let new_row = match self.selected_row {
                                    Row::Bpm => Row::Bpm,
                                    Row::FxSpeed => Row::Bpm,
                                    Row::Beats => Row::FxSpeed,
                                    Row::Trans => Row::Beats,
                                    Row::ActionPlay => Row::Trans,
                                    Row::Instrument(0) => Row::ActionPlay,
                                    Row::Instrument(i) => Row::Instrument(i - 1),
                                    Row::Envelope(0) => Row::Instrument(NUM_INSTRUMENTS - 1),
                                    Row::Envelope(i) => Row::Envelope(i - 1),
                                    Row::ActionTuning => Row::Envelope(NUM_ENVELOPES - 1),
                                    Row::ActionNewOpen => Row::ActionTuning,
                                    Row::ActionQuit => Row::ActionTuning,
                                    Row::ActionSave => Row::ActionNewOpen,
                                    Row::ActionExport => Row::ActionQuit,
                                    Row::PatternPrev => Row::ActionSave,
                                    Row::PatternNext => Row::ActionExport,
                                    Row::PatternNew => Row::PatternPrev,
                                    Row::PatternCopy => Row::PatternNew,
                                    Row::PatternDelete => Row::PatternCopy,
                                    Row::PatternZap => Row::PatternDelete,
                                };
                                self.selected_row = new_row;
                                self.selected_col = default_col(new_row);
                                self.update_focus_states();
                            }
                            KeyCode::Down => {
                                let new_row = match self.selected_row {
                                    Row::Bpm => Row::FxSpeed,
                                    Row::FxSpeed => Row::Beats,
                                    Row::Beats => Row::Trans,
                                    Row::Trans => Row::ActionPlay,
                                    Row::ActionPlay => Row::Instrument(0),
                                    Row::Instrument(i) if i < NUM_INSTRUMENTS - 1 => {
                                        Row::Instrument(i + 1)
                                    }
                                    Row::Instrument(_) => Row::Envelope(0),
                                    Row::Envelope(i) if i < NUM_ENVELOPES - 1 => {
                                        Row::Envelope(i + 1)
                                    }
                                    Row::Envelope(_) => Row::ActionTuning,
                                    Row::ActionTuning => Row::ActionNewOpen,
                                    Row::ActionNewOpen => Row::ActionSave,
                                    Row::ActionQuit => Row::ActionExport,
                                    Row::ActionSave => Row::PatternPrev,
                                    Row::ActionExport => Row::PatternNext,
                                    Row::PatternPrev => Row::PatternNew,
                                    Row::PatternNext => Row::PatternNew,
                                    Row::PatternNew => Row::PatternCopy,
                                    Row::PatternCopy => Row::PatternDelete,
                                    Row::PatternDelete => Row::PatternZap,
                                    Row::PatternZap => Row::PatternZap,
                                };
                                self.selected_row = new_row;
                                self.selected_col = default_col(new_row);
                                self.update_focus_states();
                            }
                            KeyCode::Left => {
                                let row = self.selected_row;
                                match row {
                                    Row::ActionQuit => self.selected_row = Row::ActionNewOpen,
                                    Row::ActionExport => self.selected_row = Row::ActionSave,
                                    Row::PatternNext => self.selected_row = Row::PatternPrev,
                                    Row::PatternCopy => self.selected_row = Row::PatternNew,
                                    Row::PatternDelete => self.selected_row = Row::PatternCopy,
                                    Row::PatternZap => self.selected_row = Row::PatternDelete,
                                    _ => self.selected_col = self.selected_col.prev(row),
                                }
                                self.update_focus_states();
                            }
                            KeyCode::Right => {
                                let row = self.selected_row;
                                match row {
                                    Row::ActionNewOpen => self.selected_row = Row::ActionQuit,
                                    Row::ActionSave => self.selected_row = Row::ActionExport,
                                    Row::PatternPrev => self.selected_row = Row::PatternNext,
                                    Row::PatternNew => self.selected_row = Row::PatternCopy,
                                    Row::PatternCopy => self.selected_row = Row::PatternDelete,
                                    Row::PatternDelete => self.selected_row = Row::PatternZap,
                                    _ => self.selected_col = self.selected_col.next(row),
                                }
                                self.update_focus_states();
                            }
                            KeyCode::Enter => {
                                match self.selected_col {
                                    Col::Input => self.start_editing(),
                                    Col::Plus => self.increment(file),
                                    Col::Minus => self.decrement(file),
                                    Col::Open => match self.selected_row {
                                        Row::ActionPlay => {
                                            actions.push(ComponentAction::Play);
                                        }
                                        Row::ActionNewOpen => {
                                            actions.push(ComponentAction::OpenFileBrowser);
                                        }
                                        Row::Instrument(i) => {
                                            actions
                                                .push(ComponentAction::OpenInstrumentEditor(i));
                                        }
                                        Row::Envelope(i) => {
                                            actions.push(ComponentAction::OpenEnvelopeEditor(i));
                                        }
                                        Row::ActionTuning => {
                                            actions.push(ComponentAction::OpenTuningEditor);
                                        }
                                        Row::ActionQuit => {
                                            actions.push(ComponentAction::OpenQuitConfirm);
                                        }
                                        Row::ActionSave => {
                                            actions.push(ComponentAction::SaveFile);
                                            self.save_feedback_until =
                                                Some(Instant::now() + Duration::from_secs(3));
                                        }
                                        Row::ActionExport => {
                                            actions.push(ComponentAction::Export);
                                        }
                                        Row::PatternPrev => {
                                            actions.push(ComponentAction::PatternPrev);
                                        }
                                        Row::PatternNext => {
                                            actions.push(ComponentAction::PatternNext);
                                        }
                                        Row::PatternNew => {
                                            actions.push(ComponentAction::PatternNew);
                                        }
                                        Row::PatternCopy => {
                                            actions.push(ComponentAction::PatternCopy);
                                        }
                                        Row::PatternDelete => {
                                            actions
                                                .push(ComponentAction::OpenPatternDeleteConfirm);
                                        }
                                        Row::PatternZap => {
                                            actions.push(ComponentAction::PatternZap);
                                        }
                                        _ => {}
                                    },
                                }
                                self.update_focus_states();
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        actions.extend(self.take_edit_actions());
        actions
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, file: &TrackerFile) {
        self.refresh_from_file(file);
        let bg = SCHEME.true_dark_color(SCHEME.black[3]);
        frame.render_widget(Block::new().style(Style::new().bg(bg)), area);

        let sel_row = self.selected_row;
        let sel_col = self.selected_col;
        let editing = self.editing;

        let label_w = Row::Bpm.label().len() as u16;
        let input_w = [Row::Bpm, Row::FxSpeed, Row::Beats]
            .iter()
            .map(|r| format!("{:X}", r.range().1).len() as u16)
            .max()
            .unwrap_or(3)
            .max(4);

        let inner_margin = (area.width.saturating_sub(PATTERN_TABLE_WIDTH)) / 2 + 1;
        let working_area = Rect {
            x: area.x + inner_margin,
            y: area.y,
            width: PATTERN_TABLE_WIDTH.saturating_sub(2).min(area.width),
            height: area.height,
        };

        let [settings_col, instrument_col, envelope_col, actions_col] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .areas(working_area);

        let default_style = Style::new().bg(bg).fg(SCHEME.white[2]);
        let selected_style = Style::new()
            .bg(bg)
            .fg(SCHEME.orange[3])
            .add_modifier(Modifier::BOLD);
        let btn_base = Style::new().bg(bg).fg(SCHEME.white[2]);
        let btn_focus = Style::new()
            .bg(SCHEME.orange[3])
            .fg(SCHEME.black[0])
            .add_modifier(Modifier::BOLD);

        let setting_rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .split(settings_col);

        for (row_area, row) in [
            (setting_rows[0], Row::Bpm),
            (setting_rows[1], Row::FxSpeed),
            (setting_rows[2], Row::Beats),
            (setting_rows[3], Row::Trans),
        ] {
            let row_sel = row == sel_row;
            let input_focused = row_sel && sel_col == Col::Input;
            let plus_focused = row_sel && sel_col == Col::Plus;
            let minus_focused = row_sel && sel_col == Col::Minus;

            let [label_area, input_area, _, plus_area, minus_area] = Layout::horizontal([
                Constraint::Length(label_w),
                Constraint::Length(input_w),
                Constraint::Length(1),
                Constraint::Length(BTN_W),
                Constraint::Length(BTN_W),
            ])
            .areas(row_area);

            frame.render_widget(
                Paragraph::new(row.label()).style(if row_sel {
                    selected_style
                } else {
                    default_style
                }),
                label_area,
            );

            let input_state = match row {
                Row::Bpm => &mut self.bpm_input,
                Row::FxSpeed => &mut self.fx_speed_input,
                Row::Beats => &mut self.rows_input,
                Row::Trans => &mut self.trans_input,
                _ => unreachable!(),
            };
            input_state.focus.set(input_focused && editing);
            frame.render_stateful_widget(
                TextInput::new().style(if input_focused {
                    selected_style
                } else {
                    default_style
                }),
                input_area,
                input_state,
            );
            if input_focused
                && editing
                && let Some((cx, cy)) = input_state.screen_cursor()
            {
                frame.set_cursor_position((cx, cy));
            }

            let (plus_state, minus_state) = match row {
                Row::Bpm => (&mut self.bpm_plus, &mut self.bpm_minus),
                Row::FxSpeed => (&mut self.fx_speed_plus, &mut self.fx_speed_minus),
                Row::Beats => (&mut self.rows_plus, &mut self.rows_minus),
                Row::Trans => (&mut self.trans_plus, &mut self.trans_minus),
                _ => unreachable!(),
            };
            plus_state.focus.set(plus_focused);
            minus_state.focus.set(minus_focused);
            let plus_line =
                Line::from("[+]").style(if plus_focused { btn_focus } else { btn_base });
            let minus_line =
                Line::from("[-]").style(if minus_focused { btn_focus } else { btn_base });
            frame.render_stateful_widget(
                Button::new(plus_line)
                    .style(btn_base)
                    .focus_style(btn_focus),
                plus_area,
                plus_state,
            );
            frame.render_stateful_widget(
                Button::new(minus_line)
                    .style(btn_base)
                    .focus_style(btn_focus),
                minus_area,
                minus_state,
            );
        }

        let play_focused = sel_row == Row::ActionPlay;
        let play_label = if self.playing {
            "[Pause]".to_string()
        } else if let Some(error) = self.validation_errors.first() {
            format!("[Error on Pattern {:02}]", error.pattern)
        } else {
            "[Play]".to_string()
        };
        let play_area = Rect {
            x: setting_rows[6].x,
            y: setting_rows[6].y,
            width: (play_label.len() as u16).min(setting_rows[6].width),
            height: 1,
        };
        self.action_play_btn.focus.set(play_focused);
        frame.render_stateful_widget(
            Button::new(Line::from(play_label).style(if play_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            play_area,
            &mut self.action_play_btn,
        );

        let instrument_rows =
            Layout::vertical([Constraint::Length(1); NUM_INSTRUMENTS]).split(instrument_col);
        let envelope_rows =
            Layout::vertical([Constraint::Length(1); NUM_ENVELOPES]).split(envelope_col);

        let slot_styles = SlotStyles {
            default_style,
            selected_style,
            btn_base,
            btn_focus,
        };
        let selected_instrument = match sel_row {
            Row::Instrument(i) => Some((i, sel_col)),
            _ => None,
        };
        let selected_envelope = match sel_row {
            Row::Envelope(i) => Some((i, sel_col)),
            _ => None,
        };

        render_named_slots(
            frame,
            &mut self.instruments,
            &instrument_rows,
            selected_instrument,
            editing,
            &slot_styles,
        );
        render_named_slots(
            frame,
            &mut self.envelopes,
            &envelope_rows,
            selected_envelope,
            editing,
            &slot_styles,
        );

        let action_rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .split(actions_col);

        const NEW_OPEN_BTN_W: u16 = 10;
        const QUIT_BTN_W: u16 = 6;
        const TUNING_BTN_W: u16 = 15;

        let tuning_focused = sel_row == Row::ActionTuning;
        let tuning_btn_area = Rect {
            x: action_rows[2].x,
            y: action_rows[2].y,
            width: TUNING_BTN_W.min(action_rows[2].width),
            height: 1,
        };
        self.action_tuning_btn.focus.set(tuning_focused);
        frame.render_stateful_widget(
            Button::new(Line::from("[Tuning editor]").style(if tuning_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            tuning_btn_area,
            &mut self.action_tuning_btn,
        );

        let new_open_focused = sel_row == Row::ActionNewOpen;
        let quit_focused = sel_row == Row::ActionQuit;
        let save_focused = sel_row == Row::ActionSave;
        let export_focused = sel_row == Row::ActionExport;

        let showing_saved = self.save_feedback_until.is_some_and(|t| t > Instant::now());
        let save_label = if showing_saved { "[Saved!]" } else { "[Save]" };
        let save_btn_w = save_label.len() as u16;

        let showing_exported = self
            .export_feedback_until
            .is_some_and(|t| t > Instant::now());
        let export_label = if showing_exported {
            "[Exported]"
        } else {
            "[Export]"
        };
        let export_btn_w = export_label.len() as u16;

        let new_open_w = NEW_OPEN_BTN_W.min(action_rows[0].width);
        let new_open_area = Rect {
            x: action_rows[0].x,
            y: action_rows[0].y,
            width: new_open_w,
            height: 1,
        };
        let quit_x = action_rows[0].x + new_open_w + 1;
        let quit_area = Rect {
            x: quit_x,
            y: action_rows[0].y,
            width: QUIT_BTN_W.min(action_rows[0].width.saturating_sub(new_open_w + 1)),
            height: 1,
        };
        let save_w = save_btn_w.min(action_rows[1].width);
        let save_area = Rect {
            x: action_rows[1].x,
            y: action_rows[1].y,
            width: save_w,
            height: 1,
        };
        let export_x = action_rows[1].x + save_w + 1;
        let export_area = Rect {
            x: export_x,
            y: action_rows[1].y,
            width: export_btn_w.min(action_rows[1].width.saturating_sub(save_w + 1)),
            height: 1,
        };

        self.action_new_open_btn.focus.set(new_open_focused);
        self.action_quit_btn.focus.set(quit_focused);
        self.action_save_btn.focus.set(save_focused);
        self.action_export_btn.focus.set(export_focused);

        frame.render_stateful_widget(
            Button::new(Line::from("[New/Open]").style(if new_open_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            new_open_area,
            &mut self.action_new_open_btn,
        );
        frame.render_stateful_widget(
            Button::new(Line::from("[Quit]").style(if quit_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            quit_area,
            &mut self.action_quit_btn,
        );
        frame.render_stateful_widget(
            Button::new(Line::from(save_label).style(if save_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            save_area,
            &mut self.action_save_btn,
        );
        frame.render_stateful_widget(
            Button::new(Line::from(export_label).style(if export_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            export_area,
            &mut self.action_export_btn,
        );

        let pattern_nav_row = action_rows[4];
        let pattern_prev_focused = sel_row == Row::PatternPrev;
        let pattern_next_focused = sel_row == Row::PatternNext;
        const PATTERN_NAV_BTN_W: u16 = 5;
        let [pattern_prev_area, pattern_next_area, pattern_label_area] = Layout::horizontal([
            Constraint::Length(PATTERN_NAV_BTN_W),
            Constraint::Length(PATTERN_NAV_BTN_W),
            Constraint::Fill(1),
        ])
        .areas(pattern_nav_row);

        self.pattern_prev_btn.focus.set(pattern_prev_focused);
        self.pattern_next_btn.focus.set(pattern_next_focused);
        frame.render_stateful_widget(
            Button::new(Line::from("[<--]").style(if pattern_prev_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            pattern_prev_area,
            &mut self.pattern_prev_btn,
        );
        frame.render_stateful_widget(
            Button::new(Line::from("[-->]").style(if pattern_next_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            pattern_next_area,
            &mut self.pattern_next_btn,
        );
        frame.render_widget(
            Paragraph::new(format!(" Pattern {:02}", self.pattern_idx)).style(default_style),
            pattern_label_area,
        );

        let pattern_mgmt_row = action_rows[6];
        let pattern_new_focused = sel_row == Row::PatternNew;
        let pattern_copy_focused = sel_row == Row::PatternCopy;
        let pattern_delete_focused = sel_row == Row::PatternDelete;
        let pattern_zap_focused = sel_row == Row::PatternZap;

        const PATTERN_NEW_W: u16 = 5;
        const PATTERN_COPY_W: u16 = 5;
        const PATTERN_DELETE_W: u16 = 5;
        const PATTERN_ZAP_W: u16 = 5;
        let [pattern_new_area, pattern_copy_area, pattern_delete_area, pattern_zap_area, _] =
            Layout::horizontal([
                Constraint::Length(PATTERN_NEW_W),
                Constraint::Length(PATTERN_COPY_W),
                Constraint::Length(PATTERN_DELETE_W),
                Constraint::Length(PATTERN_ZAP_W),
                Constraint::Fill(1),
            ])
            .areas(pattern_mgmt_row);

        self.pattern_new_btn.focus.set(pattern_new_focused);
        self.pattern_copy_btn.focus.set(pattern_copy_focused);
        self.pattern_delete_btn.focus.set(pattern_delete_focused);
        self.pattern_zap_btn.focus.set(pattern_zap_focused);

        frame.render_stateful_widget(
            Button::new(Line::from("[New]").style(if pattern_new_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            pattern_new_area,
            &mut self.pattern_new_btn,
        );
        frame.render_stateful_widget(
            Button::new(Line::from("[Cpy]").style(if pattern_copy_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            pattern_copy_area,
            &mut self.pattern_copy_btn,
        );
        frame.render_stateful_widget(
            Button::new(Line::from("[Del]").style(if pattern_delete_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            pattern_delete_area,
            &mut self.pattern_delete_btn,
        );
        frame.render_stateful_widget(
            Button::new(Line::from("[Zap]").style(if pattern_zap_focused {
                btn_focus
            } else {
                btn_base
            }))
            .style(btn_base)
            .focus_style(btn_focus),
            pattern_zap_area,
            &mut self.pattern_zap_btn,
        );
    }
}
