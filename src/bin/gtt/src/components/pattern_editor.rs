use ratatui::{
    Frame,
    crossterm::event::{
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    },
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Cell, Row, Table, TableState},
};

use rat_widget::choice::{Choice, ChoiceState};

use crate::{
    action::ComponentAction,
    component::Component,
    file::{TrackerFile, TuningData},
    lane::{Lane, LaneKind},
    scheme::SCHEME,
    tracker::{Beat, ChannelCmd, Pattern, SequencerCmd},
};

mod keybinds {
    use ratatui::crossterm::event::KeyCode;

    pub const NOTE_OFF: [KeyCode; 2] = [KeyCode::Char('`'), KeyCode::Char('~')];

    pub const CLEAR: [KeyCode; 2] = [KeyCode::Backspace, KeyCode::Delete];

    pub const VOL_INCREMENT: KeyCode = KeyCode::Char('=');
    pub const VOL_DECREMENT: KeyCode = KeyCode::Char('-');
}

const SEQ_CHOICE_LABELS: [&str; 4] = ["None", "Stop", "Tempo", "Speed"];

#[derive(Default, Clone, Copy)]
struct ViewLayout {
    outer: Rect,
    table: Rect,
    page_h: u16,
    scroll: usize,
}

pub struct PatternEditor {
    pub sel_x: u8,
    pub sel_y: u8,
    pub playing: bool,
    pub pattern_idx: u8,
    view_layout: ViewLayout,
    lanes: Vec<Lane>,
    transpose: i32,
    vol_edit: Option<(u8, u8)>,
    fx_edit: Option<((u8, u8), u8)>,
    seq_choice: ChoiceState<usize>,
    seq_choice_cell: Option<(u8, u8)>,
    seq_edit: Option<((u8, u8), u8)>,
}

impl PatternEditor {
    pub fn init() -> Self {
        Self {
            view_layout: ViewLayout::default(),
            playing: false,
            pattern_idx: 0,
            lanes: vec![
                Lane::beat(),
                Lane::seq(),
                Lane::note(0),
                Lane::vol(0),
                Lane::fx(0),
                Lane::note(1),
                Lane::vol(1),
                Lane::fx(1),
                Lane::note(2),
                Lane::vol(2),
                Lane::fx(2),
                Lane::note(3),
                Lane::vol(3),
                Lane::fx(3),
                Lane::note(4),
                Lane::vol(4),
                Lane::fx(4),
                Lane::note(5),
                Lane::vol(5),
                Lane::fx(5),
                Lane::note(6),
                Lane::vol(6),
                Lane::fx(6),
                Lane::note(7),
                Lane::vol(7),
                Lane::fx(7),
            ],
            transpose: 0,
            sel_x: 2,
            sel_y: 0,
            vol_edit: None,
            fx_edit: None,
            seq_choice: ChoiceState::new(),
            seq_choice_cell: None,
            seq_edit: None,
        }
    }

    pub fn set_tuning(&mut self, _tuning: TuningData) {}

    pub fn set_transpose(&mut self, transpose: i32) {
        self.transpose = transpose;
    }

    fn get_channel_beat(ch: Option<usize>, beat: u8, pattern: &Pattern) -> &Beat {
        match ch {
            Some(n) => &pattern[n + 1][beat as usize],
            None => &pattern[0][beat as usize],
        }
    }

    pub fn get_cell(&self, row: usize, column: usize, pattern: &Pattern) -> CellDisplay {
        let lane = &self.lanes[column];
        let beat = row as u8;

        match lane.kind {
            LaneKind::Beat => CellDisplay::BeatNum(beat),
            LaneKind::Seq => {
                let b = Self::get_channel_beat(lane.ch, beat, pattern);
                CellDisplay::SeqCmds(b.sqc.clone())
            }
            LaneKind::Note => {
                let b = Self::get_channel_beat(lane.ch, beat, pattern);
                let note = b.cmd_list.iter().find_map(|c| match c {
                    ChannelCmd::Note(s) => Some(NoteCell::On(s.clone())),
                    ChannelCmd::NoteOff => Some(NoteCell::Off),
                    _ => None,
                });
                CellDisplay::Note(note.unwrap_or(NoteCell::Empty))
            }
            LaneKind::Vol => {
                let b = Self::get_channel_beat(lane.ch, beat, pattern);
                let vol = b.cmd_list.iter().find_map(|c| match c {
                    ChannelCmd::Volume(v) => Some(*v),
                    _ => None,
                });
                CellDisplay::Vol(vol)
            }
            LaneKind::Fx => {
                let b = Self::get_channel_beat(lane.ch, beat, pattern);
                let arp = b.cmd_list.iter().find_map(|c| match c {
                    ChannelCmd::Arpeggio(x, y) => Some((*x, *y)),
                    _ => None,
                });
                CellDisplay::Fx(arp.map(|(x, y)| (1u8, x, y)))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CellStyle {
    EvenRow,
    OddRow,
    SelectedRow,
    SelectedCell,
    Bar,
}

pub enum CellDisplay {
    BeatNum(u8),
    SeqCmds(Option<SequencerCmd>),
    Note(NoteCell),
    Vol(Option<u8>),
    Fx(Option<(u8, u8, u8)>),
}

pub enum NoteCell {
    Empty,
    Off,
    On(String),
}

impl CellDisplay {
    fn text(&self) -> String {
        match self {
            CellDisplay::BeatNum(beat) => format!("   {:02X}", beat),
            CellDisplay::SeqCmds(cmd) => match cmd {
                None => "----".to_string(),
                Some(SequencerCmd::Stop) => " STP".to_string(),
                Some(SequencerCmd::Tempo(v)) => format!("T{:03}", v),
                Some(SequencerCmd::Speed(v)) => format!("S{:03}", v),
            },
            CellDisplay::Note(cell) => match cell {
                NoteCell::Empty => "---".to_string(),
                NoteCell::Off => "OFF".to_string(),
                NoteCell::On(s) => format!("{:<3}", s),
            },
            CellDisplay::Vol(maybe_set) => match maybe_set {
                Some(v) => format!("{:02X}", v),
                None => "--".to_string(),
            },
            CellDisplay::Fx(fx) => match fx {
                None => "---".to_string(),
                Some((id, x, y)) => format!("{:01X}{:01X}{:01X}", id, x, y),
            },
        }
    }

    fn style(&self, cell_style: CellStyle, active_pattern: bool) -> Style {
        let black = SCHEME.true_dark_color(SCHEME.black[0]);
        let mut style = SCHEME.style(black);

        let (fg, modifiers) = match self {
            CellDisplay::BeatNum(_) => (SCHEME.deepblue[2], Modifier::ITALIC),
            CellDisplay::SeqCmds(cmd) => (
                match cmd {
                    None => SCHEME.gray[0],
                    Some(SequencerCmd::Stop) => SCHEME.red[1],
                    Some(SequencerCmd::Tempo(_)) => SCHEME.deepblue[1],
                    Some(SequencerCmd::Speed(_)) => SCHEME.green[1],
                },
                Modifier::empty(),
            ),
            CellDisplay::Note(cell) => (
                match cell {
                    NoteCell::Empty => SCHEME.gray[1],
                    NoteCell::Off => SCHEME.red[1],
                    NoteCell::On(_) => SCHEME.orange[1],
                },
                Modifier::empty(),
            ),
            CellDisplay::Vol(v) => (
                match v {
                    None => SCHEME.gray[0],
                    Some(_) => SCHEME.magenta[0],
                },
                Modifier::empty(),
            ),
            CellDisplay::Fx(fx) => (
                match fx {
                    None => SCHEME.gray[0],
                    Some(_) => SCHEME.yellow[1],
                },
                Modifier::empty(),
            ),
        };

        style = style.fg(fg).add_modifier(modifiers);

        let (row_bg, add_modifiers) = match cell_style {
            CellStyle::EvenRow => (SCHEME.true_dark_color(SCHEME.black[3]), Modifier::empty()),
            CellStyle::OddRow => (SCHEME.true_dark_color(SCHEME.black[0]), Modifier::empty()),
            CellStyle::SelectedRow => (SCHEME.true_dark_color(SCHEME.blue[0]), Modifier::empty()),
            CellStyle::SelectedCell => {
                style = style.fg(SCHEME.deepblue[1]);
                (
                    SCHEME.true_dark_color(SCHEME.blue[3]),
                    Modifier::SLOW_BLINK | Modifier::REVERSED,
                )
            }
            CellStyle::Bar => todo!(),
        };

        let style = style.bg(row_bg).add_modifier(add_modifiers);

        if active_pattern {
            style
        } else {
            style.fg(SCHEME.true_dark_color(SCHEME.white[2]))
        }
    }

    fn spans(&self, lane: &Lane, style: CellStyle, is_active: bool) -> Vec<Span<'static>> {
        let (left_pad, right_pad) = lane.padding;

        let pad_style = if style == CellStyle::SelectedCell {
            self.style(CellStyle::SelectedRow, is_active)
        } else {
            self.style(style, is_active)
        };

        let pre = Span::from(" ".repeat(left_pad as usize)).style(pad_style);
        let post = Span::from(" ".repeat(right_pad as usize)).style(pad_style);
        let val = Span::from(self.text()).style(self.style(style, is_active));

        vec![pre, val, post]
    }
}

impl Component for PatternEditor {
    fn update(&mut self, events: Vec<Event>, file: &mut TrackerFile) -> Vec<ComponentAction> {
        if self.playing {
            return Vec::new();
        }
        let seq_popup_active = self.seq_choice.is_popup_active();
        if !seq_popup_active {
            for event in &events {
                let Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) = event
                else {
                    continue;
                };
            match code {
                KeyCode::Up => {
                    self.sel_y = if self.sel_y == 0 { 63 } else { self.sel_y - 1 };
                    self.vol_edit = None;
                    self.fx_edit = None;
                    self.seq_edit = None;
                    self.seq_choice_cell = None;
                }
                KeyCode::Down => {
                    self.sel_y = if self.sel_y == 63 { 0 } else { self.sel_y + 1 };
                    self.vol_edit = None;
                    self.fx_edit = None;
                    self.seq_edit = None;
                    self.seq_choice_cell = None;
                }
                KeyCode::Left => {
                    self.sel_x = if self.sel_x == 0 {
                        self.lanes.len() as u8 - 1
                    } else {
                        self.sel_x - 1
                    };
                    self.vol_edit = None;
                    self.fx_edit = None;
                    self.seq_edit = None;
                    self.seq_choice_cell = None;
                }
                KeyCode::Right => {
                    self.sel_x = (self.sel_x + 1) % self.lanes.len() as u8;
                    self.vol_edit = None;
                    self.fx_edit = None;
                    self.seq_edit = None;
                    self.seq_choice_cell = None;
                }
                KeyCode::PageUp => {
                    let step = (self.view_layout.page_h / 2).max(1) as u8;
                    self.sel_y = self.sel_y.saturating_sub(step);
                    self.vol_edit = None;
                    self.fx_edit = None;
                    self.seq_edit = None;
                    self.seq_choice_cell = None;
                }
                KeyCode::PageDown => {
                    let step = (self.view_layout.page_h / 2).max(1) as u8;
                    self.sel_y = (self.sel_y + step).min(63);
                    self.vol_edit = None;
                    self.fx_edit = None;
                    self.seq_edit = None;
                    self.seq_choice_cell = None;
                }
                _ => {}
            }
            }
        }

        let lane = &self.lanes[self.sel_x as usize];
        let (lane_kind, ch) = (lane.kind, lane.ch);
        if let (LaneKind::Note, Some(channel)) = (lane_kind, ch) {
            let scale_size = file.tuning.scale.len();
            let note_keys: Vec<String> = file.tuning.notes.keys().cloned().collect();
            let shift = self.transpose * scale_size as i32;
            for event in &events {
                match event {
                    Event::Key(KeyEvent {
                        code,
                        kind: KeyEventKind::Press,
                        ..
                    }) if keybinds::NOTE_OFF.contains(code) => {
                        let row = self.sel_y as usize;
                        let pattern = file.current_pattern_mut(self.pattern_idx);
                        let beat = &mut pattern[channel + 1][row];
                        beat.cmd_list
                            .retain(|c| !matches!(c, ChannelCmd::Note(_) | ChannelCmd::NoteOff));
                        beat.cmd_list.push(ChannelCmd::NoteOff);
                        self.sel_y = (self.sel_y + 1) % 64;
                    }
                    Event::Key(KeyEvent {
                        code,
                        kind: KeyEventKind::Press,
                        ..
                    }) if keybinds::CLEAR.contains(code) => {
                        let row = self.sel_y as usize;
                        let pattern = file.current_pattern_mut(self.pattern_idx);
                        let beat = &mut pattern[channel + 1][row];
                        beat.cmd_list
                            .retain(|c| !matches!(c, ChannelCmd::Note(_) | ChannelCmd::NoteOff));
                    }
                    Event::Key(KeyEvent {
                        code: KeyCode::Char(c),
                        kind: KeyEventKind::Press,
                        modifiers,
                        ..
                    }) if matches!(*modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                        let key_str = c.to_string();
                        let base_name = file
                            .tuning
                            .key_assignments
                            .iter()
                            .find(|(_, keys)| keys.iter().any(|k| k == &key_str))
                            .map(|(note, _)| note.clone());
                        if let Some(name) = base_name {
                            let base_idx = note_keys.iter().position(|k| k == &name);
                            if let Some(idx) = base_idx {
                                let shifted = idx as i32 + shift;
                                if shifted >= 0 && shifted < note_keys.len() as i32 {
                                    let transposed = note_keys[shifted as usize].clone();
                                    let row = self.sel_y as usize;
                                    let pattern = file.current_pattern_mut(self.pattern_idx);
                                    let beat = &mut pattern[channel + 1][row];
                                    beat.cmd_list.retain(|c| {
                                        !matches!(c, ChannelCmd::Note(_) | ChannelCmd::NoteOff)
                                    });
                                    beat.cmd_list.push(ChannelCmd::Note(transposed));
                                    self.sel_y = (self.sel_y + 1) % 64;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if let (LaneKind::Vol, Some(channel)) = (lane_kind, ch) {
            let cell = (self.sel_x, self.sel_y);
            let row = self.sel_y as usize;
            for event in &events {
                match event {
                    Event::Key(KeyEvent {
                        code: KeyCode::Char(c),
                        kind: KeyEventKind::Press,
                        modifiers,
                        ..
                    }) if matches!(*modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
                        && c.is_ascii_hexdigit() =>
                    {
                        let digit = c.to_digit(16).unwrap() as u8;
                        let prev = if self.vol_edit == Some(cell) {
                            let pattern = file.current_pattern(self.pattern_idx);
                            pattern[channel + 1][row]
                                .cmd_list
                                .iter()
                                .find_map(|c| match c {
                                    ChannelCmd::Volume(v) => Some(*v),
                                    _ => None,
                                })
                                .unwrap_or(0)
                        } else {
                            0
                        };
                        let value = ((prev % 16) * 16 + digit).min(0x3F);
                        self.vol_edit = Some(cell);

                        let pattern = file.current_pattern_mut(self.pattern_idx);
                        let beat = &mut pattern[channel + 1][row];
                        beat.cmd_list
                            .retain(|c| !matches!(c, ChannelCmd::Volume(_)));
                        beat.cmd_list.push(ChannelCmd::Volume(value));
                    }
                    Event::Key(KeyEvent {
                        code,
                        kind: KeyEventKind::Press,
                        modifiers,
                        ..
                    }) if matches!(*modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
                        && (*code == keybinds::VOL_INCREMENT
                            || *code == keybinds::VOL_DECREMENT) =>
                    {
                        self.vol_edit = None;
                        let pattern = file.current_pattern(self.pattern_idx);
                        let current =
                            pattern[channel + 1][row]
                                .cmd_list
                                .iter()
                                .find_map(|cmd| match cmd {
                                    ChannelCmd::Volume(v) => Some(*v),
                                    _ => None,
                                });

                        let next_value = if *code == keybinds::VOL_INCREMENT {
                            match current {
                                None => Some(0),
                                Some(v) if v < 0x3F => Some(v + 1),
                                Some(_) => None,
                            }
                        } else {
                            match current {
                                Some(v) if v > 0 => Some(v - 1),
                                _ => None,
                            }
                        };

                        if let Some(value) = next_value {
                            let pattern = file.current_pattern_mut(self.pattern_idx);
                            let beat = &mut pattern[channel + 1][row];
                            beat.cmd_list
                                .retain(|c| !matches!(c, ChannelCmd::Volume(_)));
                            beat.cmd_list.push(ChannelCmd::Volume(value));
                        }
                    }
                    Event::Key(KeyEvent {
                        code,
                        kind: KeyEventKind::Press,
                        ..
                    }) if keybinds::CLEAR.contains(code) => {
                        self.vol_edit = None;
                        let pattern = file.current_pattern_mut(self.pattern_idx);
                        let beat = &mut pattern[channel + 1][row];
                        beat.cmd_list
                            .retain(|c| !matches!(c, ChannelCmd::Volume(_)));
                    }
                    _ => {}
                }
            }
        }

        if let (LaneKind::Fx, Some(channel)) = (lane_kind, ch) {
            let cell = (self.sel_x, self.sel_y);
            let row = self.sel_y as usize;
            for event in &events {
                match event {
                    Event::Key(KeyEvent {
                        code: KeyCode::Char(c),
                        kind: KeyEventKind::Press,
                        modifiers,
                        ..
                    }) if matches!(*modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
                        && c.is_ascii_hexdigit() =>
                    {
                        let digit = c.to_digit(16).unwrap() as u8;
                        let (mut fx_id, mut fx_x, mut fx_y, digits_typed) =
                            match self.fx_edit {
                                Some((c, n)) if c == cell => {
                                    let pattern = file.current_pattern(self.pattern_idx);
                                    let (id, x, y) = pattern[channel + 1][row]
                                        .cmd_list
                                        .iter()
                                        .find_map(|c| match c {
                                            ChannelCmd::Arpeggio(x, y) => Some((1u8, *x, *y)),
                                            _ => None,
                                        })
                                        .unwrap_or((0, 0, 0));
                                    (id, x, y, n)
                                }
                                _ => (0, 0, 0, 0),
                            };

                        match digits_typed {
                            0 => {
                                if digit > 1 {
                                    continue;
                                }
                                fx_id = digit;
                            }
                            1 => fx_x = digit,
                            _ => fx_y = digit,
                        }
                        let digits_typed = (digits_typed + 1).min(3);
                        self.fx_edit = Some((cell, digits_typed));

                        let pattern = file.current_pattern_mut(self.pattern_idx);
                        let beat = &mut pattern[channel + 1][row];
                        beat.cmd_list
                            .retain(|c| !matches!(c, ChannelCmd::Arpeggio(_, _)));
                        if fx_id == 1 {
                            beat.cmd_list.push(ChannelCmd::Arpeggio(fx_x, fx_y));
                        }
                    }
                    Event::Key(KeyEvent {
                        code,
                        kind: KeyEventKind::Press,
                        ..
                    }) if keybinds::CLEAR.contains(code) => {
                        let digits_typed = match self.fx_edit {
                            Some((c, n)) if c == cell => n,
                            _ => 3,
                        };

                        if digits_typed == 0 {
                            self.fx_edit = None;
                        } else {
                            let remaining = digits_typed - 1;

                            let (fx_id, fx_x, fx_y) = if remaining == 0 {
                                self.fx_edit = None;
                                (0, 0, 0)
                            } else {
                                self.fx_edit = Some((cell, remaining));
                                let pattern = file.current_pattern(self.pattern_idx);
                                let (id, x, y) = pattern[channel + 1][row]
                                    .cmd_list
                                    .iter()
                                    .find_map(|c| match c {
                                        ChannelCmd::Arpeggio(x, y) => Some((1u8, *x, *y)),
                                        _ => None,
                                    })
                                    .unwrap_or((0, 0, 0));
                                match remaining {
                                    2 => (id, x, 0),
                                    _ => (id, 0, 0),
                                }
                            };

                            let pattern = file.current_pattern_mut(self.pattern_idx);
                            let beat = &mut pattern[channel + 1][row];
                            beat.cmd_list
                                .retain(|c| !matches!(c, ChannelCmd::Arpeggio(_, _)));
                            if fx_id == 1 {
                                beat.cmd_list.push(ChannelCmd::Arpeggio(fx_x, fx_y));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if let (LaneKind::Seq, None) = (lane_kind, ch) {
            let cell = (self.sel_x, self.sel_y);
            let row = self.sel_y as usize;
            let current_sqc = file.current_pattern(self.pattern_idx)[0][row].sqc.clone();

            if self.seq_choice.is_popup_active() && self.seq_choice_cell == Some(cell) {
                for event in &events {
                    let Event::Key(KeyEvent {
                        code,
                        kind: KeyEventKind::Press,
                        ..
                    }) = event
                    else {
                        continue;
                    };
                    match code {
                        KeyCode::Up => {
                            let cur = self.seq_choice.value();
                            if cur > 0 {
                                self.seq_choice.set_value(cur - 1);
                            }
                        }
                        KeyCode::Down => {
                            let cur = self.seq_choice.value();
                            self.seq_choice
                                .set_value((cur + 1).min(SEQ_CHOICE_LABELS.len() - 1));
                        }
                        KeyCode::Enter | KeyCode::Esc => {
                            self.seq_choice.set_popup_active(false);
                            self.seq_choice_cell = None;
                            let idx = self.seq_choice.value();
                            let new_sqc = match idx {
                                1 => Some(SequencerCmd::Stop),
                                2 => Some(SequencerCmd::Tempo(0)),
                                3 => Some(SequencerCmd::Speed(0)),
                                _ => None,
                            };
                            let pattern = file.current_pattern_mut(self.pattern_idx);
                            pattern[0][row].sqc = new_sqc;
                        }
                        _ => {}
                    }
                }
            } else {
                for event in &events {
                    match event {
                        Event::Key(KeyEvent {
                            code: KeyCode::Enter | KeyCode::Char(' '),
                            kind: KeyEventKind::Press,
                            ..
                        }) => {
                            let idx = match current_sqc {
                                None => 0,
                                Some(SequencerCmd::Stop) => 1,
                                Some(SequencerCmd::Tempo(_)) => 2,
                                Some(SequencerCmd::Speed(_)) => 3,
                            };
                            self.seq_choice.set_value(idx);
                            self.seq_choice.set_popup_active(true);
                            self.seq_choice_cell = Some(cell);
                        }
                        Event::Key(KeyEvent {
                            code: KeyCode::Char(c),
                            kind: KeyEventKind::Press,
                            modifiers,
                            ..
                        }) if matches!(*modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
                            && c.is_ascii_digit() =>
                        {
                            let digit = c.to_digit(10).unwrap() as u8;
                            let digits_typed = match self.seq_edit {
                                Some((c, n)) if c == cell => n,
                                _ => 0,
                            };

                            let prev = match &current_sqc {
                                Some(SequencerCmd::Tempo(v)) => *v,
                                Some(SequencerCmd::Speed(v)) => *v,
                                _ => 0,
                            };
                            let prev = if digits_typed == 0 { 0 } else { prev };
                            let value = (prev as u32 * 10 + digit as u32).min(255) as u8;
                            self.seq_edit = Some((cell, (digits_typed + 1).min(3)));

                            let new_sqc = match &current_sqc {
                                Some(SequencerCmd::Tempo(_)) => {
                                    Some(SequencerCmd::Tempo(value.min(255)))
                                }
                                Some(SequencerCmd::Speed(_)) => {
                                    Some(SequencerCmd::Speed(value.min(31)))
                                }
                                other => other.clone(),
                            };
                            let pattern = file.current_pattern_mut(self.pattern_idx);
                            pattern[0][row].sqc = new_sqc;
                        }
                        Event::Key(KeyEvent {
                            code,
                            kind: KeyEventKind::Press,
                            ..
                        }) if keybinds::CLEAR.contains(code) => {
                            self.seq_edit = None;
                            let pattern = file.current_pattern_mut(self.pattern_idx);
                            pattern[0][row].sqc = None;
                        }
                        _ => {}
                    }
                }
            }
        }

        Vec::new()
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, file: &TrackerFile) {
        let table_width: u16 = self.lanes.iter().map(|l| l.width).sum();
        let cols = Layout::default()
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(table_width),
                Constraint::Fill(1),
            ])
            .direction(Direction::Horizontal)
            .split(area);

        let table_area = cols[1];
        let per_page = table_area.height.saturating_sub(1) as usize;
        let sel = self.sel_y as usize;
        let scroll = if per_page == 0 || 64 <= per_page {
            0
        } else {
            sel.saturating_sub(per_page / 2).min(64 - per_page)
        };
        self.view_layout = ViewLayout {
            outer: area,
            table: table_area,
            page_h: per_page as u16,
            scroll,
        };

        let ch_colors = [
            SCHEME.red[3],
            SCHEME.orange[3],
            SCHEME.yellow[3],
            SCHEME.green[3],
            SCHEME.deepblue[3],
            SCHEME.blue[3],
            SCHEME.purple[3],
            SCHEME.magenta[3],
        ];

        let header_cells: Vec<Cell> = self
            .lanes
            .iter()
            .map(|lane| {
                let span = match lane.kind {
                    LaneKind::Beat => Span::from(lane.title.clone()),
                    LaneKind::Seq => Span::from(lane.title.clone()),
                    LaneKind::Note => Span::from(lane.title.clone())
                        .fg(ch_colors[lane.ch.unwrap()])
                        .italic(),
                    LaneKind::Vol => Span::from(lane.title.clone()).fg(ch_colors[lane.ch.unwrap()]),
                    LaneKind::Fx => Span::from(lane.title.clone()).fg(ch_colors[lane.ch.unwrap()]),
                };
                Cell::from(span)
            })
            .collect();
        let header = Row::new(header_cells).style(SCHEME.true_dark_black(0));

        let lane_count = self.lanes.len();
        let pattern = file.current_pattern(self.pattern_idx);
        let cell_data: Vec<Vec<CellDisplay>> = (0..64)
            .map(|row| {
                (0..lane_count)
                    .map(|col| self.get_cell(row, col, pattern))
                    .collect()
            })
            .collect();

        let rows: Vec<Row> = (0..64)
            .map(|table_row| {
                let row_even = table_row % 2 == 0;
                let row_selected = table_row == sel;

                let cells: Vec<Cell> = (0..lane_count)
                    .map(|col| {
                        let lane = &self.lanes[col];
                        let col_selected = col == self.sel_x as usize;
                        let style = if row_selected {
                            if col_selected {
                                CellStyle::SelectedCell
                            } else {
                                CellStyle::SelectedRow
                            }
                        } else if row_even {
                            CellStyle::EvenRow
                        } else {
                            CellStyle::OddRow
                        };
                        let spans = cell_data[table_row][col].spans(lane, style, true);
                        Cell::from(Line::from(spans))
                    })
                    .collect();
                Row::new(cells)
            })
            .collect();

        let widths: Vec<Constraint> = self
            .lanes
            .iter()
            .map(|l| Constraint::Length(l.width))
            .collect();
        let table = Table::new(rows, widths)
            .column_spacing(0)
            .header(header)
            .style(SCHEME.true_dark_black(0).fg(SCHEME.white[0]));

        let mut ts = TableState::default().with_offset(scroll);
        frame.render_stateful_widget(table, table_area, &mut ts);
    }
}

impl PatternEditor {
    pub fn render_popup(&mut self, frame: &mut Frame, boundary: Rect) {
        if !self.seq_choice.is_popup_active() {
            return;
        }
        let Some((sel_x, sel_y)) = self.seq_choice_cell else {
            return;
        };
        let header_bottom = self.view_layout.table.y + 1;
        let visible_row = (sel_y as usize).checked_sub(self.view_layout.scroll);
        let Some(visible_row) = visible_row else {
            return;
        };
        let cell_y = header_bottom + visible_row as u16;
        if cell_y < header_bottom || cell_y >= self.view_layout.table.y + self.view_layout.table.height
        {
            return;
        }

        let mut cell_x = self.view_layout.table.x;
        for lane in self.lanes.iter().take(sel_x as usize) {
            cell_x += lane.width;
        }
        let lane = &self.lanes[sel_x as usize];
        let cell_area = Rect {
            x: cell_x,
            y: cell_y,
            width: lane.width,
            height: 1,
        };

        let items: Vec<(usize, Line)> = SEQ_CHOICE_LABELS
            .iter()
            .enumerate()
            .map(|(i, label)| (i, Line::from(*label)))
            .collect();
        let bg = SCHEME.true_dark_color(SCHEME.black[3]);
        let (main, popup) = Choice::new()
            .items(items)
            .style(Style::new().bg(bg).fg(SCHEME.white[2]))
            .select_style(
                Style::default()
                    .bg(SCHEME.orange[3])
                    .fg(SCHEME.black[0])
                    .add_modifier(Modifier::BOLD),
            )
            .popup_len(SEQ_CHOICE_LABELS.len() as u16)
            .popup_boundary(boundary)
            .into_widgets();
        frame.render_stateful_widget(&main, cell_area, &mut self.seq_choice);
        frame.render_stateful_widget(popup, cell_area, &mut self.seq_choice);
    }
}

impl PatternEditor {
    pub fn on_mouse_event(&mut self, event: &Event) -> bool {
        let Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            ..
        }) = event
        else {
            return false;
        };
        let pos = Position {
            x: *column,
            y: *row,
        };

        if !self.view_layout.outer.contains(pos) {
            return false;
        }

        if self.view_layout.table.contains(pos) {
            let header_bottom = self.view_layout.table.y + 1;
            if pos.y >= header_bottom {
                let clicked_row = (pos.y - header_bottom) as usize + self.view_layout.scroll;
                self.sel_y = clicked_row.min(63) as u8;

                let mut col_x = self.view_layout.table.x;
                for (i, lane) in self.lanes.iter().enumerate() {
                    if pos.x >= col_x && pos.x < col_x + lane.width {
                        self.sel_x = i as u8;
                        break;
                    }
                    col_x += lane.width;
                }
            }
        }

        true
    }
}

