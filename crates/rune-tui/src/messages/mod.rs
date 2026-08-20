mod render;

use std::time::Duration;

use ratatui::layout::Rect;
use rune_core::buffer::Buffer;
use rune_core::coords::DisplayRow;

use crate::app::App;
use crate::commands::clipboard::{extract_copy_text, write_to_clipboard_or_report};
use crate::commands::mouse::{WHEEL_ROWS, extend_drag_cursor, place_click_cursor};
use crate::commands::mouse_hit::hit_test;
use crate::document::{Document, ReadOnly};
use crate::focus::{self, FocusTarget};
use crate::keymap::{Command, KeyCode, KeyInput};
use crate::pane::Pane;
use crate::pointer::{Drag, MouseButton, MouseInput, MouseKind};
use crate::runtime::Effects;

pub use render::draw;

const MAX_ENTRIES: usize = 200;

const EMPTY_TEXT: &str = "\u{b7} no messages";

pub const AUTO_COLLAPSE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warn,
    Error,
}

impl Severity {
    fn glyph(self) -> &'static str {
        match self {
            Severity::Error => "\u{26a0} ",
            Severity::Warn => "! ",
            Severity::Info => "\u{b7} ",
        }
    }
}

pub struct Message {
    pub severity: Severity,
    pub text: String,
}

pub struct MessageLog {
    entries: Vec<Message>,
    doc: Document,
    ranges: Vec<(std::ops::Range<usize>, Severity)>,
    open: bool,
    armed: Option<crate::generation::MessagesCollapseGen>,
    generation: crate::generation::GenCounter<crate::generation::MessagesCollapse>,
    pinned: bool,
    posts: u64,
}

impl MessageLog {
    pub fn new() -> MessageLog {
        let mut doc = Document::new(Buffer::new(EMPTY_TEXT));
        doc.read_only = ReadOnly::Always;
        doc.focused = false;
        MessageLog {
            entries: Vec::new(),
            doc,
            ranges: Vec::new(),
            open: false,
            armed: None,
            generation: crate::generation::GenCounter::default(),
            pinned: true,
            posts: 0,
        }
    }
}

impl Default for MessageLog {
    fn default() -> MessageLog {
        MessageLog::new()
    }
}

fn sanitize(text: &str) -> String {
    // C1 controls (U+0080-U+009F) decode from UTF-8 as ordinary two-byte
    // sequences, unlike C0/DEL which are single-byte, so a filesystem path
    // or error string can carry one without it looking like a raw control
    // byte on the wire.
    text.chars()
        .filter(|&c| {
            c == '\n'
                || !(('\u{0}'..='\u{1f}').contains(&c)
                    || c == '\u{7f}'
                    || ('\u{80}'..='\u{9f}').contains(&c))
        })
        .collect()
}

fn build_markdown(entries: &[Message]) -> (String, Vec<(std::ops::Range<usize>, Severity)>) {
    if entries.is_empty() {
        return (EMPTY_TEXT.to_string(), Vec::new());
    }
    let mut text = String::new();
    let mut ranges = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            text.push_str("\n\n");
        }
        let start = text.len();
        text.push_str(entry.severity.glyph());
        text.push_str(&entry.text);
        ranges.push((start..text.len(), entry.severity));
    }
    (text, ranges)
}

fn rebuild_doc(app: &mut App) {
    let (text, ranges) = build_markdown(&app.messages.entries);
    let focused = app.messages.doc.focused;
    let scroll_row = app.messages.doc.viewport.scroll_row;
    let width = app.messages.doc.viewport.width;
    let mut doc = Document::new(Buffer::new(text));
    doc.read_only = ReadOnly::Always;
    doc.focused = focused;
    doc.viewport.scroll_row = scroll_row;
    doc.viewport.width = width;
    app.messages.doc = doc;
    app.messages.ranges = ranges;
}

pub fn post(app: &mut App, severity: Severity, text: impl Into<String>) {
    let text = sanitize(&text.into());
    app.messages.entries.push(Message { severity, text });
    app.messages.posts = app.messages.posts.wrapping_add(1);
    if app.messages.entries.len() > MAX_ENTRIES {
        let overflow = app.messages.entries.len() - MAX_ENTRIES;
        app.messages.entries.drain(0..overflow);
    }
    rebuild_doc(app);
    app.messages.open = true;
    app.messages.armed = None;
    app.messages.pinned = true;
}

pub fn info(app: &mut App, text: impl Into<String>) {
    post(app, Severity::Info, text);
}

pub fn warn(app: &mut App, text: impl Into<String>) {
    post(app, Severity::Warn, text);
}

pub fn error(app: &mut App, text: impl Into<String>) {
    post(app, Severity::Error, text);
}

pub fn toggle(app: &mut App, effects: &mut Effects) {
    if !app.messages.open {
        app.messages.open = true;
        focus(app, effects);
    } else if app.focus() == Pane::Messages {
        collapse(app);
        app.set_focus_pane(Pane::Editor, effects);
    } else {
        focus(app, effects);
    }
}

fn focus(app: &mut App, effects: &mut Effects) {
    app.messages.doc.focused = true;
    app.messages.armed = None;
    app.set_focus_pane(Pane::Messages, effects);
}

pub fn collapse(app: &mut App) {
    app.messages.open = false;
    app.messages.doc.focused = false;
    app.messages.armed = None;
}

pub fn is_open(app: &App) -> bool {
    app.messages.open
}

pub fn doc(app: &App) -> &Document {
    &app.messages.doc
}

pub fn doc_mut(app: &mut App) -> &mut Document {
    &mut app.messages.doc
}

pub fn newest(app: &App) -> Option<&Message> {
    app.messages.entries.last()
}

pub fn newest_text(app: &App) -> Option<&str> {
    newest(app).map(|m| m.text.as_str())
}

pub fn posts(app: &App) -> u64 {
    app.messages.posts
}

pub fn log_text(app: &App) -> String {
    app.messages
        .entries
        .iter()
        .map(|m| m.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn content_rows(app: &App, frame_height: u16) -> u16 {
    let total = app
        .messages
        .doc
        .view
        .as_ref()
        .map_or(0, |v| v.display.total_rows());
    let cap = ((frame_height as usize) * 2 / 5).max(1);
    total.min(cap).max(1) as u16
}

pub fn height(app: &App, frame_height: u16) -> u16 {
    if !app.messages.open {
        return 0;
    }
    content_rows(app, frame_height) + 1
}

pub fn sync(app: &mut App, width: u16, frame_height: u16) {
    if !app.messages.open {
        return;
    }
    app.messages.doc.viewport.width = width;
    let view = app.messages.doc.sync();
    app.messages.doc.view = Some(view);
    let height = content_rows(app, frame_height);
    app.messages.doc.viewport.height = height;
    if app.messages.pinned {
        let total_rows = app
            .messages
            .doc
            .view
            .as_ref()
            .map_or(0, |v| v.display.total_rows());
        app.messages.doc.viewport.scroll_row =
            DisplayRow(total_rows.saturating_sub(height as usize));
    }
}

fn page_amount(app: &App) -> isize {
    app.messages.doc.viewport.height.max(1) as isize
}

fn scroll(app: &mut App, delta: isize) {
    app.messages.pinned = false;
    let max_row = app
        .messages
        .doc
        .view
        .as_ref()
        .map_or(DisplayRow(usize::MAX), |v| {
            DisplayRow(v.display.total_rows().saturating_sub(1))
        });
    let scroll_row = app.messages.doc.viewport.scroll_row;
    app.messages.doc.viewport.scroll_row = if delta >= 0 {
        (scroll_row + delta as usize).min(max_row)
    } else {
        scroll_row - (-delta) as usize
    };
}

pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> bool {
    match key.code {
        KeyCode::Escape => {
            collapse(app);
            app.set_focus_pane(Pane::Editor, effects);
            true
        }
        KeyCode::Up => {
            scroll(app, -1);
            true
        }
        KeyCode::Down => {
            scroll(app, 1);
            true
        }
        KeyCode::PageUp => {
            scroll(app, -page_amount(app));
            true
        }
        KeyCode::PageDown => {
            scroll(app, page_amount(app));
            true
        }
        _ if crate::keymap::resolve(key) == Some(Command::Copy) => {
            copy_selection(app, effects);
            true
        }
        _ => false,
    }
}

pub fn should_arm_auto_collapse(app: &App) -> bool {
    app.messages.open
        && app.messages.armed.is_none()
        && focus::target(app) != FocusTarget::Messages
        && !app.messages.doc.cursors.primary().has_selection()
        && !matches!(newest(app), Some(m) if m.severity == Severity::Error)
}

pub fn arm_auto_collapse(app: &mut App) -> crate::generation::MessagesCollapseGen {
    let generation = app.messages.generation.mint();
    app.messages.armed = Some(generation);
    generation
}

pub fn is_armed(app: &App, generation: crate::generation::MessagesCollapseGen) -> bool {
    app.messages.armed == Some(generation)
}

pub fn is_collapse_armed(app: &App) -> bool {
    app.messages.armed.is_some()
}

fn pane_rect(app: &App) -> Option<Rect> {
    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    crate::layout::geometry(area, app).messages
}

fn relative(app: &App, input: MouseInput) -> Option<(u16, u16)> {
    let rect = pane_rect(app)?;
    let rel_row = input.row.checked_sub(rect.y)?;
    if rel_row == 0 || rel_row >= rect.height {
        return None;
    }
    let col = input.column.saturating_sub(rect.x);
    Some((rel_row - 1, col))
}

pub fn mouse(app: &mut App, input: MouseInput, effects: &mut Effects) {
    match input.kind {
        MouseKind::ScrollUp => scroll(app, -WHEEL_ROWS),
        MouseKind::ScrollDown => scroll(app, WHEEL_ROWS),
        MouseKind::Down(MouseButton::Left) => mouse_down(app, input, effects),
        MouseKind::Drag(MouseButton::Left) => mouse_drag(app, input),
        MouseKind::Up(MouseButton::Left) => copy_selection(app, effects),
        _ => {}
    }
}

fn mouse_down(app: &mut App, input: MouseInput, effects: &mut Effects) {
    focus(app, effects);
    let Some((row, col)) = relative(app, input) else {
        return;
    };
    let Some((offset, desired_col)) = hit_test(app, &app.messages.doc, row, col) else {
        return;
    };

    let now = app.clock.now();
    let count = app.pointer.register_click(now, input.column, input.row);

    if place_click_cursor(&mut app.messages.doc, offset, desired_col, count) {
        app.pointer.drag = Some(Drag::Text {
            anchor: offset,
            pane: Pane::Messages,
        });
    } else {
        app.pointer.drag = None;
    }
}

fn mouse_drag(app: &mut App, input: MouseInput) {
    let Some(Drag::Text {
        anchor,
        pane: Pane::Messages,
    }) = app.pointer.drag
    else {
        return;
    };
    let Some((row, col)) = relative(app, input) else {
        return;
    };
    let Some((offset, desired_col)) = hit_test(app, &app.messages.doc, row, col) else {
        return;
    };
    extend_drag_cursor(&mut app.messages.doc, anchor, offset, desired_col);
}

fn copy_selection(app: &mut App, effects: &mut Effects) {
    if !app.messages.doc.cursors.primary().has_selection() {
        return;
    }
    let text = extract_copy_text(&app.messages.doc.buffer, &app.messages.doc.cursors);
    write_to_clipboard_or_report(app, &text, effects);
}
