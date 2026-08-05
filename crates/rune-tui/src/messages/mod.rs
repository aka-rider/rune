//! The message log: an append-only, severity-tagged record of every
//! transient user-facing message, rendered in a collapsible read-only pane
//! directly above the footer (plan WP1). `messages::post` (and its
//! `info`/`warn`/`error` wrappers) is the ONE chokepoint every such message
//! funnels through — replacing both the old modal error banner (which
//! captured every key) and the pre-WP4 single status slot with its own
//! save-failure-provenance clearing rule. A log needs neither: nothing is
//! ever cleared, and the pane never takes focus on its own.
//!
//! Split for the §1.6 budget: this file holds the log's state and its
//! `&mut App`/`&App` API; [`render`] holds the pane's own row builder.

mod render;

use std::time::Duration;

use ratatui::layout::Rect;
use rune_core::buffer::Buffer;
use rune_core::cursor::{Cursor, CursorSet};

use crate::app::App;
use crate::commands::clipboard::{extract_copy_text, write_to_clipboard_or_report};
use crate::commands::mouse::{WHEEL_ROWS, select_range};
use crate::commands::mouse_hit::hit_test;
use crate::commands::nav::word_range_at;
use crate::commands::nav_line::line_range_incl_newline;
use crate::document::{Document, ReadOnly};
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::pane::Pane;
use crate::pointer::{Drag, MouseButton, MouseInput, MouseKind};
use crate::runtime::{Cmd, CmdKind, Effects, Msg};

pub use render::draw;

/// Pushing past this many entries drops the oldest — an unbounded log would
/// leak for the lifetime of a long session.
const MAX_ENTRIES: usize = 200;

const EMPTY_TEXT: &str = "\u{b7} no messages";

/// The pane's auto-collapse delay (plan WP2, Assumption A2) — armed by
/// `dispatch::after_update`, not by [`post`] itself (decision 9): `post`
/// only takes `&mut App`, and threading `&mut Effects` through every call
/// site is not worth it for a timer that can just as well be armed once,
/// after the whole message batch settles.
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

/// The log's own state: every posted entry, the read-only `Document` its
/// text is rendered through (so the pane reuses `render::build_rows`
/// exactly like the editor does, rather than a second cell-building path),
/// and whether the pane is currently open.
pub struct MessageLog {
    entries: Vec<Message>,
    doc: Document,
    /// The byte range each entry occupies in `doc`'s buffer, alongside its
    /// severity — `render::draw`'s own colour pass keys off this instead of
    /// re-deriving it from the entry list every frame.
    ranges: Vec<(std::ops::Range<usize>, Severity)>,
    open: bool,
    /// The generation of the currently in-flight auto-collapse timer, or
    /// `None` while none is armed (plan WP2) — set by [`arm_auto_collapse`],
    /// cleared by anything that must suppress or restart the countdown
    /// (posting, focusing the pane, collapsing).
    armed: Option<u32>,
    /// The next generation [`arm_auto_collapse`] will hand out. Monotonic
    /// for the app's lifetime, so a superseded timer's `Msg` (an older
    /// generation arriving after a newer one was armed) is always
    /// distinguishable from the current one.
    generation: u32,
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
            generation: 0,
        }
    }
}

impl Default for MessageLog {
    fn default() -> MessageLog {
        MessageLog::new()
    }
}

/// Strips C0 control bytes (except `\n`) and DEL from `text` (plan WP1
/// decision 8): error text can carry filesystem-derived bytes, and those
/// bytes are blitted to the terminal and OSC-52-copied verbatim, so they
/// must never reach the rendered grid unsanitized.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|&c| c == '\n' || !(('\u{0}'..='\u{1f}').contains(&c) || c == '\u{7f}'))
        .collect()
}

/// The document buffer's markdown source: every sanitized entry, glyph-
/// prefixed, joined as separate paragraphs (`"\n\n"`) so comrak keeps each
/// on its own display row — plus the byte range each entry landed at, for
/// `render`'s severity-colour pass. `EMPTY_TEXT` when there are no entries
/// at all.
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

/// The one chokepoint every transient message funnels through: sanitizes
/// `text`, appends it (dropping the oldest past [`MAX_ENTRIES`]), rebuilds
/// the log's document, and opens the pane. Never focuses it (plan WP1
/// decision 3 — a message is not modal): typing continues to reach whatever
/// already had focus.
pub fn post(app: &mut App, severity: Severity, text: impl Into<String>) {
    let text = sanitize(&text.into());
    app.messages.entries.push(Message { severity, text });
    if app.messages.entries.len() > MAX_ENTRIES {
        let overflow = app.messages.entries.len() - MAX_ENTRIES;
        app.messages.entries.drain(0..overflow);
    }
    rebuild_doc(app);
    app.messages.open = true;
    // A new message restarts the countdown (plan WP2.S4): `after_update`'s
    // reconciler re-arms from scratch on the next settle, now against the
    // NEWEST entry's severity.
    app.messages.armed = None;
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

/// Opens (and focuses) the pane if it's closed; collapses it if it's open
/// and already focused; otherwise (open but unfocused) just focuses it —
/// the `^E`/`⌘E` toggle's whole state machine (plan WP1.S7).
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
    // A focused pane must never auto-collapse out from under the user (plan
    // WP2.S4); `after_update`'s reconciler will not re-arm while focused.
    app.messages.armed = None;
    app.set_focus_pane(Pane::Messages, effects);
}

/// Closes the pane without moving focus — the caller decides where focus
/// goes next (`toggle`/`handle_key`'s `Esc` arm both do). Also clears any
/// armed auto-collapse timer: a stale generation still fires harmlessly
/// (the timeout handler no-ops on a mismatched generation), but there is
/// nothing left for it to collapse.
pub fn collapse(app: &mut App) {
    app.messages.open = false;
    app.messages.doc.focused = false;
    app.messages.armed = None;
}

pub fn is_open(app: &App) -> bool {
    app.messages.open
}

/// The log's own read-only document — exposed so a caller that already
/// works generically over `&Document`/`&mut Document` (`render::build_rows`,
/// WP3's hit-testing/copy, and tests exercising the pane's cursor state
/// directly) can reach it without a matching accessor per field.
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

/// Every entry's sanitized text, oldest first, joined by `\n` with no
/// glyphs — the test-assertion helper (plan Risks: the mechanical
/// `footer_text` -> helper swap WP4 needs).
pub fn log_text(app: &App) -> String {
    app.messages
        .entries
        .iter()
        .map(|m| m.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The content rows available to the pane at `frame_height`, capped at the
/// 40% end of the requested 30-40% band — always at least 1 (even an empty
/// log shows its one `EMPTY_TEXT` line).
fn content_rows(app: &App, frame_height: u16) -> u16 {
    let total = app
        .messages
        .doc
        .view
        .as_ref()
        .map(|v| v.display.total_rows())
        .unwrap_or(0);
    let cap = ((frame_height as usize) * 2 / 5).max(1);
    total.min(cap).max(1) as u16
}

/// The pane's total rendered height at `frame_height`: `0` when closed, else
/// one separator row plus [`content_rows`].
pub fn height(app: &App, frame_height: u16) -> u16 {
    if !app.messages.open {
        return 0;
    }
    content_rows(app, frame_height) + 1
}

/// Re-syncs the log document at `width`/`frame_height` — mirrors what
/// `App::sync_view` already does for the active document, from the same
/// settle step. Must run before `App::relayout`, which sizes the editor
/// viewport from a rect that has [`height`] carved out of it, and [`height`]
/// is read off the very view this refreshes; syncing afterwards leaves the
/// editor's viewport trailing the pane by one pass. A no-op while the pane is
/// closed: there is nothing on screen to keep in sync.
pub fn sync(app: &mut App, width: u16, frame_height: u16) {
    if !app.messages.open {
        return;
    }
    app.messages.doc.viewport.width = width;
    let view = app.messages.doc.sync();
    app.messages.doc.view = Some(view);
    app.messages.doc.viewport.height = content_rows(app, frame_height);
}

fn page_amount(app: &App) -> isize {
    app.messages.doc.viewport.height.max(1) as isize
}

fn scroll(app: &mut App, delta: isize) {
    let max_row = app
        .messages
        .doc
        .view
        .as_ref()
        .map(|v| v.display.total_rows().saturating_sub(1))
        .unwrap_or(usize::MAX);
    let current = app.messages.doc.viewport.scroll_row as isize;
    let next = (current + delta).max(0) as usize;
    app.messages.doc.viewport.scroll_row = next.min(max_row);
}

/// The pane's own key handling, reached from stage 3 of the key pipeline
/// while `app.focus() == Pane::Messages`. `Esc` collapses and returns focus
/// to the editor; the arrow/page keys scroll. Returns whether the key was
/// consumed.
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
        KeyCode::Char('c') if is_copy_chord(key.mods) => {
            copy_selection(app, effects);
            true
        }
        _ => false,
    }
}

/// Whether `dispatch::after_update`'s reconciler should arm a fresh
/// auto-collapse timer this settle (plan WP2.S2) — every one of the four
/// suppression rules is a `false` here: the pane must be open with nothing
/// already armed, unfocused, carrying no selection on its primary cursor,
/// and its newest entry must not be an error — a data-risk message must
/// stay visible until dismissed.
pub fn should_arm_auto_collapse(app: &App) -> bool {
    app.messages.open
        && app.messages.armed.is_none()
        && app.focus() != Pane::Messages
        && !app.messages.doc.cursors.primary().has_selection()
        && !matches!(newest(app), Some(m) if m.severity == Severity::Error)
}

/// Arms a fresh auto-collapse timer: bumps the generation, marks it armed,
/// and returns the generation the caller must both tag the returned `Cmd`
/// with and push into `effects.cmds` — the same "hand the caller the token,
/// let it push the `Cmd`" shape as `save::trigger_save`'s degraded-confirm
/// arm.
pub fn arm_auto_collapse(app: &mut App) -> u32 {
    let generation = app.messages.generation;
    app.messages.generation = app.messages.generation.wrapping_add(1);
    app.messages.armed = Some(generation);
    generation
}

/// Whether `generation` is still the currently armed one — `false` for a
/// superseded or already-cleared timer, which the caller must then treat as
/// a no-op (plan WP2.S3).
pub fn is_armed(app: &App, generation: u32) -> bool {
    app.messages.armed == Some(generation)
}

/// The pane's 5s auto-collapse timer (plan WP2.S1), modelled on
/// `save::save_confirm_timeout_cmd`/`pane::quit_confirm_timeout_cmd`: sleeps
/// on its own dedicated `Cmd` thread, then hands back the generation it was
/// armed with so a superseded timer is ignored on arrival.
pub fn collapse_timeout_cmd(generation: u32) -> Cmd {
    Cmd::new(CmdKind::MessagesCollapseTimeout, move || {
        std::thread::sleep(AUTO_COLLAPSE);
        Some(Msg::MessagesCollapseTimeout { generation })
    })
}

/// `⌘C` (sup-only) or `^⇧C` (ctrl+shift) — the exact two chords the editor's
/// own `Copy` row binds (`keymap::editor_bindings::clipboard`), so the pane
/// is keyboard-copyable with the identical gesture (plan WP3.S5).
fn is_copy_chord(m: Mods) -> bool {
    (m.sup && !m.ctrl && !m.alt && !m.shift) || (m.ctrl && m.shift && !m.alt && !m.sup)
}

/// The pane's own `Rect` this frame, or `None` while it's closed — every
/// mouse handler below re-derives it fresh rather than trusting a value a
/// caller might have computed against a stale frame.
fn pane_rect(app: &App) -> Option<Rect> {
    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    crate::layout::geometry(area, app).messages
}

/// `(row, col)` relative to the pane's CONTENT area — i.e. below its one
/// separator row — for an absolute-coordinate `input`. `None` when the pane
/// isn't open, or the point lands on the separator row itself (chrome, not
/// content).
fn relative(app: &App, input: MouseInput) -> Option<(u16, u16)> {
    let rect = pane_rect(app)?;
    let rel_row = input.row.checked_sub(rect.y)?;
    if rel_row == 0 || rel_row >= rect.height {
        return None;
    }
    let col = input.column.saturating_sub(rect.x);
    Some((rel_row - 1, col))
}

/// Dispatches one `MouseInput` that belongs to the pane — either a fresh
/// press/wheel-tick landing inside its `Rect` (`commands::mouse::handle`'s
/// own rect dispatch) or a `Drag`/`Up` continuing a gesture that started
/// there, routed by the LATCHED drag's own pane rather than by wherever the
/// pointer currently sits (`Drag::Text`'s own docs) — the same reason a
/// splitter drag latches instead of re-testing its band on every event.
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

/// Left-press: focuses the pane (never modal — a message pane click is just
/// another way to reach it, exactly like `^E`), places or extends the
/// selection at the hit-tested offset, and starts a `Drag::Text { pane:
/// Messages, .. }` for a plain single click — mirrors `commands::mouse::
/// handle_left_down`'s shape for the editor, reusing the same word/line
/// range helpers so double/triple-click behave identically in both panes.
fn mouse_down(app: &mut App, input: MouseInput, effects: &mut Effects) {
    focus(app, effects);
    let Some((row, col)) = relative(app, input) else {
        return;
    };
    let Some((offset, desired_col)) = hit_test(app, &app.messages.doc, row, col) else {
        return;
    };

    let now = app.pointer_clock.now();
    let count = app.pointer.register_click(now, input.column, input.row);

    match count {
        1 => {
            let placed = Cursor {
                position: offset,
                anchor: offset,
                desired_col,
                id: 0,
            };
            app.messages.doc.cursors = CursorSet::new_from(&[placed]);
            app.pointer.drag = Some(Drag::Text {
                anchor: offset,
                pane: Pane::Messages,
            });
        }
        2 => {
            let (start, end) = word_range_at(&app.messages.doc.buffer, offset);
            select_range(&mut app.messages.doc, start, end);
            app.pointer.drag = None;
        }
        _ => {
            let (start, end) = line_range_incl_newline(&app.messages.doc.buffer, offset);
            select_range(&mut app.messages.doc, start, end);
            app.pointer.drag = None;
        }
    }
}

/// Extends the pane's selection for a latched `Drag::Text { pane: Messages,
/// .. }` — a no-op once the drag belongs to a different pane (guarded by
/// the pattern match itself) or once the pointer has left the pane's own
/// content rows (`relative` returning `None`).
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
    let id = app.messages.doc.cursors.primary().id;
    let extended = Cursor {
        position: offset,
        anchor,
        desired_col,
        id,
    };
    app.messages.doc.cursors = CursorSet::new_from(&[extended]);
}

/// Copies the pane's current selection through the same capped OSC-52 path
/// every other copy in the app uses — a no-op with no selection, so
/// `extract_copy_text`'s whole-line fallback (meant for the editor's own
/// `Copy` command with no selection) never fires here (plan Gotchas). The
/// one chokepoint both the mouse-release path and `⌘C`/`^⇧C` in
/// `handle_key` reach through, so the two can never drift on when a copy
/// actually happens.
fn copy_selection(app: &mut App, effects: &mut Effects) {
    if !app.messages.doc.cursors.primary().has_selection() {
        return;
    }
    let text = extract_copy_text(&app.messages.doc.buffer, &app.messages.doc.cursors);
    write_to_clipboard_or_report(app, &text, effects);
}
