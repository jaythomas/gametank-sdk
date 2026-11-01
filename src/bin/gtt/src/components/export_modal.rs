use std::io;
use std::path::PathBuf;

use rat_widget::button::{Button, ButtonState};
use ratatui::{
    Frame,
    crossterm::event::{
        Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
    },
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use crate::export::{TRACK_BANK_SIZE, TrackBin};
use crate::scheme::SCHEME;
use crate::{action::ComponentAction, component::Component, file::TrackerFile};

const MODAL_W: u16 = 64;
const CANCEL_W: u16 = 8;
const EXPORT_W: u16 = 8;
const BTN_GAP: u16 = 4;
const MAX_LIST_ROWS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModalState {
    Confirm,
    WriteFailed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Btn {
    Cancel,
    Export,
}

struct TrackSizes {
    total: usize,
    patterns: Vec<usize>,
}

pub struct ExportModal {
    state: ModalState,
    focused: Btn,
    path: PathBuf,
    dir_exists: bool,
    track: Result<TrackSizes, String>,
    scroll: usize,
    cancel_button: ButtonState,
    export_button: ButtonState,
}

impl ExportModal {
    pub fn init() -> Self {
        Self {
            state: ModalState::Confirm,
            focused: Btn::Cancel,
            path: PathBuf::new(),
            dir_exists: false,
            track: Err(String::new()),
            scroll: 0,
            cancel_button: ButtonState::new(),
            export_button: ButtonState::new(),
        }
    }

    pub fn open(&mut self, path: PathBuf, track: io::Result<TrackBin>) {
        self.state = ModalState::Confirm;
        self.dir_exists = path.exists();
        self.path = path;
        self.track = track
            .map(|bin| TrackSizes {
                total: bin.bytes.len(),
                patterns: bin.pattern_sizes,
            })
            .map_err(|e| e.to_string());
        self.scroll = 0;
        self.focused = if self.can_export() && !self.dir_exists {
            Btn::Export
        } else {
            Btn::Cancel
        };
    }

    pub fn set_error(&mut self) {
        self.state = ModalState::WriteFailed;
        self.focused = Btn::Cancel;
    }

    fn can_export(&self) -> bool {
        self.state == ModalState::Confirm
            && self
                .track
                .as_ref()
                .is_ok_and(|track| track.total <= TRACK_BANK_SIZE)
    }
}

fn wrapped_rows(text: &str, width: usize) -> usize {
    let width = width.max(1);
    let mut rows = 1;
    let mut used = 0;
    for word in text.split(' ') {
        let len = word.chars().count();
        let gap = usize::from(used > 0);
        if used + gap + len <= width {
            used += gap + len;
        } else if len <= width {
            rows += 1;
            used = len;
        } else {
            if used > 0 {
                rows += 1;
            }
            rows += (len - 1) / width;
            used = len - (len - 1) / width * width;
        }
    }
    rows
}

impl Component for ExportModal {
    fn update(&mut self, events: Vec<Event>, _file: &mut TrackerFile) -> Vec<ComponentAction> {
        let mut actions = Vec::new();
        for event in &events {
            match event {
                Event::Key(KeyEvent {
                    code: KeyCode::Esc,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    actions.push(ComponentAction::CloseModal);
                    break;
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Left,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    self.focused = Btn::Cancel;
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Right,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if self.can_export() {
                        self.focused = Btn::Export;
                    }
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Up,
                    kind: KeyEventKind::Press,
                    ..
                })
                | Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    ..
                }) => {
                    self.scroll = self.scroll.saturating_sub(1);
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Down,
                    kind: KeyEventKind::Press,
                    ..
                })
                | Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    ..
                }) => {
                    self.scroll += 1;
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Enter,
                    kind: KeyEventKind::Press,
                    ..
                }) => match self.focused {
                    Btn::Cancel => {
                        actions.push(ComponentAction::CloseModal);
                        break;
                    }
                    Btn::Export => {
                        if self.can_export() {
                            actions.push(ComponentAction::ConfirmExport(self.path.clone()));
                        }
                    }
                },
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
                    if self.cancel_button.area.contains(pos) {
                        actions.push(ComponentAction::CloseModal);
                        break;
                    } else if self.export_button.area.contains(pos) && self.can_export() {
                        actions.push(ComponentAction::ConfirmExport(self.path.clone()));
                    }
                }
                _ => {}
            }
        }
        actions
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _file: &TrackerFile) {
        let width = MODAL_W.min(area.width);
        let total_rows = self.track.as_ref().map_or(0, |track| track.patterns.len());
        let list_rows = total_rows.min(MAX_LIST_ROWS);
        let scrollable = total_rows > list_rows;
        self.scroll = self.scroll.min(total_rows - list_rows);

        let text_w = width.saturating_sub(3) as usize;
        let notice_text = if self.state == ModalState::WriteFailed {
            Some("Could not write directory.".to_string())
        } else if self.dir_exists {
            let name = self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Some(format!("\"{name}\" already exists and will be overwritten"))
        } else {
            None
        };
        let notice_rows = notice_text
            .as_ref()
            .map_or(0, |text| wrapped_rows(text, text_w));
        let error_text = self.track.as_ref().err().cloned();
        let error_rows = error_text
            .as_ref()
            .map_or(0, |text| wrapped_rows(text, text_w));

        let body_rows = if self.track.is_ok() {
            1 + list_rows
        } else {
            error_rows
        };
        let inner_h = body_rows + 2 + if notice_rows > 0 { notice_rows + 1 } else { 0 };
        let height = (inner_h as u16 + 2).min(area.height);

        let modal_area = Rect {
            x: area.x + area.width.saturating_sub(width) / 2,
            y: area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        };

        let bg = SCHEME.true_dark_color(SCHEME.black[2]);
        let default_style = Style::new().bg(bg).fg(SCHEME.white[2]);
        let focus_style = Style::new()
            .bg(SCHEME.orange[3])
            .fg(SCHEME.black[0])
            .add_modifier(Modifier::BOLD);
        let text_style = Style::new().bg(bg).fg(SCHEME.white[3]);
        let error_style = Style::new().bg(bg).fg(SCHEME.red[2]);
        let warn_style = Style::new().bg(bg).fg(SCHEME.orange[2]);

        frame.render_widget(Clear, modal_area);

        let block = Block::bordered()
            .border_style(Style::new().fg(SCHEME.orange[2]))
            .style(Style::new().bg(bg));
        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let mut y = inner.y;
        let mut put = |frame: &mut Frame, text: String, style: Style, rows: u16| {
            let rect = Rect {
                x: inner.x + 1,
                y,
                width: inner.width.saturating_sub(1),
                height: rows.min(inner.bottom().saturating_sub(y)),
            };
            y += rows;
            if rect.height > 0 {
                frame.render_widget(
                    Paragraph::new(text).style(style).wrap(Wrap { trim: false }),
                    rect,
                );
            }
        };

        match &self.track {
            Ok(track) => {
                let (size_line, size_style) = if track.total <= TRACK_BANK_SIZE {
                    (
                        format!(
                            "Size: {} / {} bytes ({}%)",
                            track.total,
                            TRACK_BANK_SIZE,
                            track.total * 100 / TRACK_BANK_SIZE
                        ),
                        text_style,
                    )
                } else {
                    (
                        format!(
                            "Size: {} / {} bytes, {} over the bank limit",
                            track.total,
                            TRACK_BANK_SIZE,
                            track.total - TRACK_BANK_SIZE
                        ),
                        error_style,
                    )
                };
                put(frame, size_line, size_style, 1);

                for (idx, size) in track
                    .patterns
                    .iter()
                    .enumerate()
                    .skip(self.scroll)
                    .take(list_rows)
                {
                    put(
                        frame,
                        format!("Pattern {idx:02}: {size} bytes"),
                        text_style,
                        1,
                    );
                }

                if scrollable {
                    let mut scrollbar_state = ScrollbarState::new(total_rows - list_rows + 1)
                        .position(self.scroll)
                        .viewport_content_length(list_rows);
                    frame.render_stateful_widget(
                        Scrollbar::new(ScrollbarOrientation::VerticalRight)
                            .style(Style::new().bg(bg).fg(SCHEME.orange[2])),
                        Rect {
                            x: inner.x,
                            y: inner.y + 1,
                            width: inner.width,
                            height: (list_rows as u16)
                                .min(inner.bottom().saturating_sub(inner.y + 1)),
                        },
                        &mut scrollbar_state,
                    );
                }
            }
            Err(_) => put(
                frame,
                error_text.unwrap_or_default(),
                error_style,
                error_rows as u16,
            ),
        }

        put(frame, String::new(), default_style, 1);

        if let Some(text) = notice_text {
            let style = if self.state == ModalState::WriteFailed {
                error_style
            } else {
                warn_style
            };
            put(frame, text, style, notice_rows as u16);
            put(frame, String::new(), default_style, 1);
        }

        if y >= inner.bottom() {
            return;
        }

        let btn_row_y = y;

        if !self.can_export() {
            self.focused = Btn::Cancel;
        }
        let cancel_focused = self.focused == Btn::Cancel;
        let export_focused = self.focused == Btn::Export;

        self.cancel_button.focus.set(cancel_focused);
        self.export_button.focus.set(export_focused);

        if self.can_export() {
            let total_btn_w = CANCEL_W + BTN_GAP + EXPORT_W;
            let btn_x_start = inner.x + inner.width.saturating_sub(total_btn_w) / 2;

            let cancel_area = Rect {
                x: btn_x_start,
                y: btn_row_y,
                width: CANCEL_W,
                height: 1,
            };
            let export_area = Rect {
                x: btn_x_start + CANCEL_W + BTN_GAP,
                y: btn_row_y,
                width: EXPORT_W,
                height: 1,
            };

            frame.render_stateful_widget(
                Button::new(Line::from("[Cancel]").style(if cancel_focused {
                    focus_style
                } else {
                    default_style
                }))
                .style(default_style)
                .focus_style(focus_style),
                cancel_area,
                &mut self.cancel_button,
            );
            frame.render_stateful_widget(
                Button::new(Line::from("[Export]").style(if export_focused {
                    focus_style
                } else {
                    default_style
                }))
                .style(default_style)
                .focus_style(focus_style),
                export_area,
                &mut self.export_button,
            );
        } else {
            let cancel_area = Rect {
                x: inner.x + inner.width.saturating_sub(CANCEL_W) / 2,
                y: btn_row_y,
                width: CANCEL_W,
                height: 1,
            };
            frame.render_stateful_widget(
                Button::new(Line::from("[Cancel]").style(if cancel_focused {
                    focus_style
                } else {
                    default_style
                }))
                .style(default_style)
                .focus_style(focus_style),
                cancel_area,
                &mut self.cancel_button,
            );
        }
    }
}
