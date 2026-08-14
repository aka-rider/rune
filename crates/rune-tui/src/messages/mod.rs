//! The message log: an append-only, severity-tagged record of every
//! transient user-facing message, rendered in a collapsible read-only pane
//! directly above the footer. `messages::post` (and its
//! `info`/`warn`/`error` wrappers) is the ONE chokepoint every such message
//! funnels through — replacing both the old modal error banner (which
//! captured every key) and the old single status slot with its own
//! save-failure-provenance clearing rule. A log needs neither: nothing is
//! ever cleared, and the pane never takes focus on its own.
//!
//! Split to stay under the source-file line budget: this file holds the
//! log's state and its `&mut App`/`&App` API; [`render`] holds the pane's
//! own row builder.

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
use crate::runtime::{Cmd, Effects, Msg};

pub use render::draw;

/// Pushing past this many entries drops the oldest — an unbounded log would
/// leak for the lifetime of a long session.
const MAX_ENTRIES: usize = 200;

const EMPTY_TEXT: &str = "\u{b7} no messages";

/// The pane's auto-collapse delay — armed by `dispatch::after_update`, not
/// by [`post`] itself: `post` only takes `&mut App`, and threading `&mut
/// Effects` through every call site is not worth it for a timer that can
/// just as well be armed once, after the whole message batch settles.
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
    /// `None` while none is armed — set by [`arm_auto_collapse`],
    /// cleared by anything that must suppress or restart the countdown
    /// (posting, focusing the pane, collapsing).
    armed: Option<u32>,
    /// The next generation [`arm_auto_collapse`] will hand out. Monotonic
    /// for the app's lifetime, so a superseded timer's `Msg` (an older
    /// generation arriving after a newer one was armed) is always
    /// distinguishable from the current one.
    generation: u32,
    /// Whether [`sync`] should pin `viewport.scroll_row` to the tail of the
    /// log rather than leave it where it last settled. Starts (and every
    /// [`post`] resets it back to) `true` — a message the user cannot see
    /// is not feedback — and only [`scroll`] clears it, when the user
    /// explicitly moves the viewport themselves to read back through the
    /// log; the next post still snaps them back to the newest entry.
    pinned: bool,
    /// Every call to [`post`], ever — monotonic for the app's lifetime,
    /// unlike `entries.len()`, which [`MAX_ENTRIES`] eviction can shrink
    /// back down. A caller that needs to know "was a message posted since
    /// I last looked", not "is the newest entry different from before",
    /// must compare this counter: two consecutive posts of identical text
    /// (e.g. the same hint fired by two different unbound keys in a row)
    /// leave `entries.last()` looking unchanged even though a new row
    /// landed in the pane.
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
            generation: 0,
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

/// Strips C0 control bytes (except `\n`), DEL, and the C1 control range
/// (`U+0080`-`U+009F`) from `text`: error text can carry filesystem-derived
/// bytes, and those bytes are blitted to the
/// terminal and OSC-52-copied verbatim, so they must never reach the
/// rendered grid unsanitized. C1 controls decode from UTF-8 as ordinary
/// two-byte sequences (unlike C0/DEL, which are single-byte), so a
/// filesystem path or error string can carry one without it ever looking
/// like a raw control byte on the wire.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|&c| {
            c == '\n'
                || !(('\u{0}'..='\u{1f}').contains(&c)
                    || c == '\u{7f}'
                    || ('\u{80}'..='\u{9f}').contains(&c))
        })
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
/// the log's document, and opens the pane. Never focuses it — a message is
/// not modal: typing continues to reach whatever already had focus.
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
    // A new message restarts the countdown: `after_update`'s
    // reconciler re-arms from scratch on the next settle, now against the
    // NEWEST entry's severity.
    app.messages.armed = None;
    // A message the user cannot see is not feedback: even if the user had
    // scrolled away to read older entries, a fresh post snaps the pane
    // back to the newest one on the next `sync` (finding 1).
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

/// Opens (and focuses) the pane if it's closed; collapses it if it's open
/// and already focused; otherwise (open but unfocused) just focuses it —
/// the `^E`/`⌘E` toggle's whole state machine.
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
    // A focused pane must never auto-collapse out from under the user;
    // `after_update`'s reconciler will not re-arm while focused.
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

/// The total number of [`post`] calls this app has ever made — monotonic,
/// unaffected by [`MAX_ENTRIES`] eviction. See [`MessageLog::posts`].
pub fn posts(app: &App) -> u64 {
    app.messages.posts
}

/// Every entry's sanitized text, oldest first, joined by `\n` with no
/// glyphs — the test-assertion helper for reading the log's content back
/// out without depending on the pane's own markdown rendering.
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
        .map_or(0, |v| v.display.total_rows());
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
    let height = content_rows(app, frame_height);
    app.messages.doc.viewport.height = height;
    // Top-anchored rendering (`viewport::visible_rows`) means a scroll_row
    // left wherever it last settled can leave a freshly posted message
    // entirely off-screen (finding 1) — pinned mode keeps it at the tail
    // instead, recomputed fresh every settle so a later post (or a rewrap
    // that changes `total_rows`) is still reflected. `scroll` clears
    // `pinned` for an explicit user scroll; `post` always restores it.
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
    // An explicit scroll means the user is reading back through the log —
    // stop chasing the tail until the next post (finding 1).
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
        _ if crate::keymap::resolve(key) == Some(Command::Copy) => {
            copy_selection(app, effects);
            true
        }
        _ => false,
    }
}

/// Whether `dispatch::after_update`'s reconciler should arm a fresh
/// auto-collapse timer this settle — every one of the four
/// suppression rules is a `false` here: the pane must be open with nothing
/// already armed, unfocused, carrying no selection on its primary cursor,
/// and its newest entry must not be an error — a data-risk message must
/// stay visible until dismissed.
pub fn should_arm_auto_collapse(app: &App) -> bool {
    app.messages.open
        && app.messages.armed.is_none()
        && focus::target(app) != FocusTarget::Messages
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
/// a no-op.
pub fn is_armed(app: &App, generation: u32) -> bool {
    app.messages.armed == Some(generation)
}

/// The pane's 5s auto-collapse timer, modelled on
/// `save::save_confirm_timeout_cmd`/`pane::quit_confirm_timeout_cmd`: sleeps
/// on its own dedicated `Cmd` thread, then hands back the generation it was
/// armed with so a superseded timer is ignored on arrival.
pub fn collapse_timeout_cmd(generation: u32) -> Cmd {
    Cmd::messages_collapse_timeout(move || {
        std::thread::sleep(AUTO_COLLAPSE);
        Some(Msg::MessagesCollapseTimeout { generation })
    })
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
/// another way to reach it, exactly like `^E`), then hands the hit-tested
/// offset and click count to `commands::mouse::place_click_cursor` — the
/// same click-count -> cursor shape the editor's own `handle_left_down`
/// reaches through, so double/triple-click behave identically in both
/// panes.
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
    extend_drag_cursor(&mut app.messages.doc, anchor, offset, desired_col);
}

/// Copies the pane's current selection through the same capped OSC-52 path
/// every other copy in the app uses — a no-op with no selection, so
/// `extract_copy_text`'s whole-line fallback (meant for the editor's own
/// `Copy` command with no selection) never fires here. The
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
