use std::path::Path;

use rat_widget::button::{Button, ButtonState};
use rat_widget::text::HasScreenCursor;
use rat_widget::text_input::{TextInput, TextInputState, handle_events};
use ratatui::{
    Frame,
    buffer::Buffer,
    crossterm::event::{
        Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
    },
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Clear, Paragraph},
};

use super::file_picker::{FilePicker, PickerOutcome};
use crate::scheme::SCHEME;
use crate::tracker::{ENVELOPE_MAX_STEPS, MAX_VOLUME};
use crate::{action::ComponentAction, component::Component, file::TrackerFile};

const CANCEL_BTN_W: u16 = 8;
const SAVE_BTN_W: u16 = 6;
const IMPORT_BTN_W: u16 = 8;
const GENERATE_BTN_W: u16 = 10;
const BTN_GAP: u16 = 2;
const NUM_BTN_W: u16 = 3;
const FIELD_LABEL_W: u16 = 9;
const FIELD_INPUT_W: u16 = 4;
const Y_AXIS_W: u16 = 3;
const BAR_W: u16 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Length,
    Attack,
    Decay,
    Sustain,
    Release,
}

const FIELDS: [Field; 5] = [
    Field::Length,
    Field::Attack,
    Field::Decay,
    Field::Sustain,
    Field::Release,
];

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Length => "Length:  ",
            Field::Attack => "Attack:  ",
            Field::Decay => "Decay:   ",
            Field::Sustain => "Sustain: ",
            Field::Release => "Release: ",
        }
    }

    fn range(self) -> (u8, u8) {
        match self {
            Field::Length => (1, ENVELOPE_MAX_STEPS as u8),
            Field::Sustain => (0, MAX_VOLUME),
            Field::Attack | Field::Decay | Field::Release => (0, ENVELOPE_MAX_STEPS as u8),
        }
    }
}

struct NumberField {
    input: TextInputState,
    plus: ButtonState,
    minus: ButtonState,
}

impl NumberField {
    fn new(value: u8) -> Self {
        let mut input = TextInputState::new();
        input.set_value(format!("{:X}", value));
        Self {
            input,
            plus: ButtonState::new(),
            minus: ButtonState::new(),
        }
    }
}

#[derive(Default, Clone, Copy)]
struct BarLayout {
    x: u16,
    y: u16,
    h: u16,
}

pub struct EnvelopeEditor {
    pub visible: bool,
    pub envelope_idx: usize,
    envelope_name: String,
    cancel_button: ButtonState,
    save_button: ButtonState,
    import_button: ButtonState,
    generate_button: ButtonState,
    fields: [NumberField; FIELDS.len()],
    editing_field: Option<Field>,
    steps: [u8; ENVELOPE_MAX_STEPS],
    length: usize,
    selected: usize,
    painting: bool,
    bar_layout: BarLayout,
    file_picker: FilePicker,
    import_error: Option<String>,
}

impl EnvelopeEditor {
    pub fn init() -> Self {
        Self {
            visible: false,
            envelope_idx: 0,
            envelope_name: String::new(),
            cancel_button: ButtonState::new(),
            save_button: ButtonState::new(),
            import_button: ButtonState::new(),
            generate_button: ButtonState::new(),
            fields: [
                NumberField::new(1),
                NumberField::new(0),
                NumberField::new(0),
                NumberField::new(MAX_VOLUME),
                NumberField::new(0),
            ],
            editing_field: None,
            steps: [MAX_VOLUME; ENVELOPE_MAX_STEPS],
            length: 1,
            selected: 0,
            painting: false,
            bar_layout: BarLayout::default(),
            file_picker: FilePicker::new(),
            import_error: None,
        }
    }

    pub fn open(&mut self, idx: usize, name: &str, steps: &[u8]) {
        self.visible = true;
        self.envelope_idx = idx;
        self.envelope_name = name.to_string();
        self.selected = 0;
        self.painting = false;
        self.editing_field = None;
        self.file_picker.close();
        self.import_error = None;
        self.load_steps(steps);
    }

    pub fn get_steps(&self) -> Vec<u8> {
        self.steps[..self.length].to_vec()
    }

    fn load_steps(&mut self, steps: &[u8]) {
        let len = steps.len().clamp(1, ENVELOPE_MAX_STEPS);
        self.steps = [MAX_VOLUME; ENVELOPE_MAX_STEPS];
        for (slot, value) in self.steps.iter_mut().zip(steps.iter().take(len)) {
            *slot = (*value).min(MAX_VOLUME);
        }
        let last = self.steps[len - 1];
        for slot in self.steps[len..].iter_mut() {
            *slot = last;
        }
        self.set_length(len);
    }

    fn field_mut(&mut self, field: Field) -> &mut NumberField {
        let idx = FIELDS.iter().position(|f| *f == field).unwrap();
        &mut self.fields[idx]
    }

    fn field(&self, field: Field) -> &NumberField {
        let idx = FIELDS.iter().position(|f| *f == field).unwrap();
        &self.fields[idx]
    }

    fn field_value(&self, field: Field) -> u8 {
        let (min, max) = field.range();
        u8::from_str_radix(&self.field(field).input.value::<String>(), 16)
            .unwrap_or(min)
            .clamp(min, max)
    }

    fn set_field_value(&mut self, field: Field, value: u8) {
        let (min, max) = field.range();
        let clamped = value.clamp(min, max);
        self.field_mut(field)
            .input
            .set_value(format!("{:X}", clamped));
    }

    fn set_length(&mut self, length: usize) {
        self.length = length.clamp(1, ENVELOPE_MAX_STEPS);
        self.set_field_value(Field::Length, self.length as u8);
        if self.selected >= self.length {
            self.selected = self.length - 1;
        }
    }

    fn commit_field(&mut self, field: Field) {
        let value = self.field_value(field);
        self.set_field_value(field, value);
        if field == Field::Length {
            self.set_length(value as usize);
        }
        self.editing_field = None;
    }

    fn adjust_field(&mut self, field: Field, delta: i16) {
        let (min, max) = field.range();
        let next = (self.field_value(field) as i16 + delta).clamp(min as i16, max as i16) as u8;
        self.set_field_value(field, next);
        if field == Field::Length {
            self.set_length(next as usize);
        }
    }

    fn generate_adsr(&mut self) {
        let attack = self.field_value(Field::Attack) as usize;
        let decay = self.field_value(Field::Decay) as usize;
        let sustain = self.field_value(Field::Sustain);
        let release = self.field_value(Field::Release) as usize;

        let shaped = (attack + decay + release).clamp(1, ENVELOPE_MAX_STEPS);
        let length = self.length.max(shaped);
        let hold = length - shaped.min(length);

        let mut steps = Vec::with_capacity(length);
        steps.extend(ramp(0, MAX_VOLUME, attack));
        steps.extend(ramp(MAX_VOLUME, sustain, decay));
        steps.extend(std::iter::repeat_n(sustain, hold));
        steps.extend(ramp(sustain, 0, release));
        steps.truncate(length);
        if steps.is_empty() {
            steps.push(sustain);
        }

        self.load_steps(&steps);
        self.import_error = None;
    }

    fn try_import_raw(&mut self, path: &Path) {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string());

        match std::fs::read(path) {
            Err(_) => {
                self.import_error = Some(format!("Failed to read {}", filename));
            }
            Ok(bytes) if bytes.is_empty() => {
                self.import_error = Some(format!("{} is empty", filename));
            }
            Ok(bytes) => {
                self.load_steps(&bytes);
                self.selected = 0;
                self.painting = false;
                self.import_error = if bytes.len() > ENVELOPE_MAX_STEPS {
                    Some(format!(
                        "{} is {} bytes (max {}); imported truncated",
                        filename,
                        bytes.len(),
                        ENVELOPE_MAX_STEPS
                    ))
                } else {
                    None
                };
            }
        }
    }

    fn handle_field_click(&mut self, pos: Position) -> bool {
        for field in FIELDS {
            if self.field(field).plus.area.contains(pos) {
                self.adjust_field(field, 1);
                return true;
            }
            if self.field(field).minus.area.contains(pos) {
                self.adjust_field(field, -1);
                return true;
            }
            if self.field(field).input.area.contains(pos) {
                if let Some(active) = self.editing_field
                    && active != field
                {
                    self.commit_field(active);
                }
                self.editing_field = Some(field);
                return true;
            }
        }
        false
    }
}

fn ramp(from: u8, to: u8, steps: usize) -> Vec<u8> {
    if steps == 0 {
        return Vec::new();
    }
    let span = to as i32 - from as i32;
    (0..steps)
        .map(|i| (from as i32 + span * (i as i32 + 1) / steps as i32) as u8)
        .collect()
}

impl Component for EnvelopeEditor {
    fn update(&mut self, events: Vec<Event>, _file: &mut TrackerFile) -> Vec<ComponentAction> {
        let mut actions = Vec::new();

        if self.file_picker.is_active() {
            for event in &events {
                match self.file_picker.handle_event(event) {
                    PickerOutcome::Selected(path) => {
                        self.try_import_raw(&path);
                        break;
                    }
                    PickerOutcome::Cancelled => break,
                    PickerOutcome::None => {}
                }
            }
            return actions;
        }

        for event in &events {
            if let Some(field) = self.editing_field
                && let Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) = event
            {
                match code {
                    KeyCode::Esc | KeyCode::Enter => self.commit_field(field),
                    KeyCode::Char(c) if !c.is_ascii_hexdigit() => {}
                    _ => {
                        let state = &mut self.field_mut(field).input;
                        state.focus.set(true);
                        let _ = handle_events(state, true, event);
                    }
                }
                continue;
            }

            match event {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('q') | KeyCode::Char('Q'),
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    self.visible = false;
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Left,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Right,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if self.selected + 1 < self.length {
                        self.selected += 1;
                    }
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Up,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    self.steps[self.selected] =
                        self.steps[self.selected].saturating_add(1).min(MAX_VOLUME);
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Down,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    self.steps[self.selected] = self.steps[self.selected].saturating_sub(1);
                }
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column,
                    row,
                    ..
                }) => {
                    let pos = Position {
                        x: *column,
                        y: *row,
                    };
                    if self.handle_field_click(pos) {
                    } else if self.generate_button.area.contains(pos) {
                        self.generate_adsr();
                    } else if self.import_button.area.contains(pos) {
                        self.import_error = None;
                        self.file_picker.open(
                            " Select .bin file ",
                            SCHEME.orange[2],
                            SCHEME.true_dark_color(SCHEME.black[2]),
                            "bin",
                        );
                    } else if self.cancel_button.area.contains(pos) {
                        self.visible = false;
                    } else if self.save_button.area.contains(pos) {
                        actions.push(ComponentAction::EnvelopeSaved(
                            self.envelope_idx,
                            self.get_steps(),
                        ));
                        self.visible = false;
                    } else {
                        self.painting = true;
                        self.handle_bar_click(*column, *row);
                    }
                }
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Drag(MouseButton::Left),
                    column,
                    row,
                    ..
                }) => {
                    if self.painting {
                        self.handle_bar_click(*column, *row);
                    }
                }
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Up(MouseButton::Left),
                    ..
                }) => {
                    self.painting = false;
                }
                _ => {}
            }
        }
        actions
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _file: &TrackerFile) {
        if area.height < 2 {
            return;
        }
        let modal_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(1),
        };

        let bg = SCHEME.true_dark_color(SCHEME.black[2]);
        let title_style = Style::new()
            .fg(SCHEME.white[3])
            .add_modifier(Modifier::BOLD);

        frame.render_widget(Clear, modal_area);

        let title_text = format!(
            " Editing {} (envelope {}) ",
            self.envelope_name, self.envelope_idx
        );
        let block = Block::bordered()
            .title(Line::from(title_text).style(title_style))
            .border_style(Style::new().fg(SCHEME.orange[2]))
            .style(Style::new().bg(bg));

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let save_area = Rect {
            x: modal_area.right().saturating_sub(SAVE_BTN_W + 1),
            y: modal_area.y,
            width: SAVE_BTN_W,
            height: 1,
        };
        let cancel_area = Rect {
            x: save_area.x.saturating_sub(BTN_GAP + CANCEL_BTN_W),
            y: modal_area.y,
            width: CANCEL_BTN_W,
            height: 1,
        };
        let import_area = Rect {
            x: cancel_area.x.saturating_sub(BTN_GAP + IMPORT_BTN_W),
            y: modal_area.y,
            width: IMPORT_BTN_W,
            height: 1,
        };

        let default_style = Style::new().bg(bg).fg(SCHEME.white[2]);
        let focus_style = Style::new()
            .bg(SCHEME.orange[3])
            .fg(SCHEME.black[0])
            .add_modifier(Modifier::BOLD);

        self.cancel_button.focus.set(false);
        self.save_button.focus.set(false);
        self.import_button.focus.set(false);
        frame.render_stateful_widget(
            Button::new(Line::from("[Import]").style(default_style))
                .style(default_style)
                .focus_style(focus_style),
            import_area,
            &mut self.import_button,
        );
        frame.render_stateful_widget(
            Button::new(Line::from("[Cancel]").style(default_style))
                .style(default_style)
                .focus_style(focus_style),
            cancel_area,
            &mut self.cancel_button,
        );
        frame.render_stateful_widget(
            Button::new(Line::from("[Save]").style(default_style))
                .style(default_style)
                .focus_style(focus_style),
            save_area,
            &mut self.save_button,
        );

        if self.file_picker.is_active() {
            self.file_picker.render(frame, inner);
            return;
        }

        let has_error = self.import_error.is_some();
        let [fields_area, error_row, bars_area] = Layout::vertical([
            Constraint::Length(FIELDS.len() as u16),
            Constraint::Length(if has_error { 1 } else { 0 }),
            Constraint::Fill(1),
        ])
        .areas(inner);

        self.render_fields(frame, fields_area, default_style, focus_style);

        if has_error {
            let error_style = Style::new()
                .bg(bg)
                .fg(SCHEME.red[2])
                .add_modifier(Modifier::BOLD);
            frame.render_widget(
                Paragraph::new(self.import_error.as_deref().unwrap_or("")).style(error_style),
                error_row,
            );
        }

        if bars_area.height > 0 && bars_area.width > Y_AXIS_W {
            let buf = frame.buffer_mut();
            self.render_bars(buf, bars_area, bg);
        }
    }
}

impl EnvelopeEditor {
    fn render_fields(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        default_style: Style,
        focus_style: Style,
    ) {
        let rows = Layout::vertical([Constraint::Length(1); FIELDS.len()]).split(area);
        let editing_field = self.editing_field;

        for (row_idx, field) in FIELDS.iter().enumerate() {
            let [label_area, input_area, _, plus_area, minus_area, _, extra_area] =
                Layout::horizontal([
                    Constraint::Length(FIELD_LABEL_W),
                    Constraint::Length(FIELD_INPUT_W),
                    Constraint::Length(1),
                    Constraint::Length(NUM_BTN_W),
                    Constraint::Length(NUM_BTN_W),
                    Constraint::Length(BTN_GAP),
                    Constraint::Fill(1),
                ])
                .areas(rows[row_idx]);

            frame.render_widget(
                Paragraph::new(field.label()).style(default_style),
                label_area,
            );

            let is_editing = editing_field == Some(*field);
            let entry = &mut self.fields[row_idx];
            entry.input.focus.set(is_editing);
            frame.render_stateful_widget(
                TextInput::new().style(if is_editing {
                    focus_style
                } else {
                    default_style
                }),
                input_area,
                &mut entry.input,
            );
            if is_editing && let Some((cx, cy)) = entry.input.screen_cursor() {
                frame.set_cursor_position((cx, cy));
            }

            entry.plus.focus.set(false);
            entry.minus.focus.set(false);
            frame.render_stateful_widget(
                Button::new(Line::from("[+]").style(default_style))
                    .style(default_style)
                    .focus_style(focus_style),
                plus_area,
                &mut entry.plus,
            );
            frame.render_stateful_widget(
                Button::new(Line::from("[-]").style(default_style))
                    .style(default_style)
                    .focus_style(focus_style),
                minus_area,
                &mut entry.minus,
            );

            if *field == Field::Attack {
                self.generate_button.focus.set(false);
                let generate_area = Rect {
                    width: GENERATE_BTN_W.min(extra_area.width),
                    ..extra_area
                };
                frame.render_stateful_widget(
                    Button::new(Line::from("[Generate]").style(default_style))
                        .style(default_style)
                        .focus_style(focus_style),
                    generate_area,
                    &mut self.generate_button,
                );
            }
        }
    }

    fn handle_bar_click(&mut self, column: u16, row: u16) {
        let BarLayout { x, y, h } = self.bar_layout;
        if h == 0 || column < x {
            return;
        }
        let step_idx = ((column - x) / BAR_W) as usize;
        if step_idx >= self.length {
            return;
        }
        if row < y || row >= y + h {
            return;
        }
        let rows_from_bottom = (y + h - 1 - row) as u32;
        let value = (rows_from_bottom * MAX_VOLUME as u32 / (h - 1).max(1) as u32)
            .min(MAX_VOLUME as u32) as u8;
        self.steps[step_idx] = value;
        self.selected = step_idx;
    }

    fn render_bars(&mut self, buf: &mut Buffer, area: Rect, bg: Color) {
        let label_h: u16 = 1;
        let chart_h = area.height.saturating_sub(label_h);
        let chart_x = area.x + Y_AXIS_W;
        let visible_count = (area.width.saturating_sub(Y_AXIS_W) / BAR_W) as usize;

        if visible_count == 0 || chart_h == 0 {
            return;
        }

        self.bar_layout = BarLayout {
            x: chart_x,
            y: area.y,
            h: chart_h,
        };

        let y_label_style = Style::new().bg(bg).fg(SCHEME.white[2]);
        buf.set_string(area.x, area.y, format!("{:02X}", MAX_VOLUME), y_label_style);
        buf.set_string(area.x, area.y + chart_h - 1, "00", y_label_style);

        for step_idx in 0..visible_count.min(self.length) {
            let value = self.steps[step_idx];
            let is_selected = step_idx == self.selected;

            let bar_h = (value as u32 * chart_h as u32 / MAX_VOLUME as u32) as u16;
            let bar_x = chart_x + step_idx as u16 * BAR_W;
            let bar_bg = if is_selected {
                SCHEME.orange[2]
            } else {
                SCHEME.blue[1]
            };
            let bar_style = Style::new().bg(bar_bg).fg(SCHEME.white[3]);
            let empty_style = Style::new().bg(bg);
            let bar_top = area.y + chart_h.saturating_sub(bar_h);

            for row in area.y..area.y + chart_h {
                let style = if row >= bar_top {
                    bar_style
                } else {
                    empty_style
                };
                buf.set_string(bar_x, row, "  ", style);
            }

            if bar_h >= 1 {
                buf.set_string(
                    bar_x,
                    bar_top,
                    format!("{:02X}", value),
                    bar_style.add_modifier(Modifier::BOLD),
                );
            }

            let label_row = area.y + chart_h;
            if label_row < area.bottom() {
                let label_style = if is_selected {
                    Style::new().bg(bg).fg(SCHEME.orange[3])
                } else {
                    Style::new().bg(bg).fg(SCHEME.gray[1])
                };
                buf.set_string(bar_x, label_row, format!("{:02X}", step_idx), label_style);
            }
        }
    }
}
