use rat_widget::text::HasScreenCursor;
use rat_widget::text_input::{TextInput, TextInputState, handle_events};
use ratatui::{
    Frame,
    crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind},
    layout::Rect,
    style::Style,
    text::Span,
    widgets::{Block, Clear},
};

use crate::{
    action::ComponentAction, component::Component, file::TrackerFile, scheme::SCHEME,
    tracker::{PATTERN_BEATS, PATTERN_TABLE_WIDTH},
};

pub struct CommandPalette {
    pub visible: bool,
    input: TextInputState,
}

impl CommandPalette {
    pub fn init() -> Self {
        Self {
            visible: false,
            input: TextInputState::new_focused(),
        }
    }

    fn execute(&mut self) -> Vec<ComponentAction> {
        let actions = parse_command(&self.input.value::<String>());
        self.input.set_value("");
        self.visible = false;
        actions
    }
}

/// Parses an optional hexadecimal row count (`F` or `0xF`). Rejects zero and
/// anything larger than a pattern.
fn parse_row_count(arg: Option<&str>) -> Option<usize> {
    let Some(arg) = arg else { return Some(1) };
    let digits = arg.strip_prefix("0x").or(arg.strip_prefix("0X")).unwrap_or(arg);
    let count = usize::from_str_radix(digits, 16).ok()?;
    (1..=PATTERN_BEATS).contains(&count).then_some(count)
}

fn parse_pattern_offset(arg: &str) -> Option<isize> {
    let (sign, unsigned) = match arg.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", arg),
    };
    let digits = unsigned.strip_prefix("0x").or(unsigned.strip_prefix("0X")).unwrap_or(unsigned);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    isize::from_str_radix(&format!("{sign}{digits}"), 16).ok()
}

fn parse_command(input: &str) -> Vec<ComponentAction> {
    let mut parts = input.split_whitespace();
    let Some(name) = parts.next() else { return Vec::new() };
    let arg = parts.next();
    if parts.next().is_some() {
        return Vec::new();
    }
    let action = match (name, arg) {
        ("d" | "delete", _) => parse_row_count(arg).map(ComponentAction::DeleteBeats),
        ("envelope", None) => Some(ComponentAction::OpenEnvelopeEditor(0)),
        ("exp" | "export", None) => Some(ComponentAction::Export),
        ("instrument", None) => Some(ComponentAction::OpenInstrumentEditor(0)),
        ("m", None) => Some(ComponentAction::ToggleMuteChannel),
        ("mute", None) => Some(ComponentAction::MuteChannel),
        ("p" | "paste", None) => Some(ComponentAction::PasteBelow),
        ("P" | "Paste", None) => Some(ComponentAction::PasteAbove),
        ("play", None) => Some(ComponentAction::Play),
        ("pn", None) => Some(ComponentAction::PatternNext),
        ("pn", Some(arg)) => parse_pattern_offset(arg).map(ComponentAction::PatternGoTo),
        ("pp", None) => Some(ComponentAction::PatternPrev),
        ("q" | "quit", None) => Some(ComponentAction::Quit),
        ("s" | "solo", None) => Some(ComponentAction::SoloChannel),
        ("unmute", None) => Some(ComponentAction::UnmuteChannel),
        ("w" | "write", None) => Some(ComponentAction::SaveFile),
        ("wq" | "write-quit", None) => Some(ComponentAction::SaveAndQuit),
        ("y" | "yank", _) => parse_row_count(arg).map(ComponentAction::YankBeats),
        _ => None,
    };
    action.into_iter().collect()
}

impl Component for CommandPalette {
    fn update(&mut self, events: Vec<Event>, _file: &mut TrackerFile) -> Vec<ComponentAction> {
        let mut actions = Vec::new();
        for event in &events {
            if let Event::Key(KeyEvent {
                code,
                kind: KeyEventKind::Press,
                ..
            }) = event
            {
                match code {
                    KeyCode::Esc => {
                        self.input.set_value("");
                        self.visible = false;
                        continue;
                    }
                    KeyCode::Enter => {
                        actions.extend(self.execute());
                        continue;
                    }
                    _ => {}
                }
            }
            handle_events(&mut self.input, true, event);
        }
        actions
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _file: &TrackerFile) {
        let bg = SCHEME.true_dark_color(SCHEME.gray[1]);
        let style = Style::new().bg(bg).fg(SCHEME.white[3]);

        frame.render_widget(Clear, area);
        frame.render_widget(Block::new().style(style), area);

        let input_width = PATTERN_TABLE_WIDTH.min(area.width);
        let input_x = area.x + (area.width.saturating_sub(input_width)) / 2;
        let input_area = Rect {
            x: input_x + 1,
            y: area.y,
            width: input_width,
            height: 1,
        };

        frame.render_widget(
            Span::from(":"),
            Rect {
                x: input_x,
                ..input_area
            },
        );
        let widget = TextInput::new().style(style);
        frame.render_stateful_widget(widget, input_area, &mut self.input);

        if let Some((cx, cy)) = self.input.screen_cursor() {
            frame.set_cursor_position((cx, cy));
        }
    }
}
