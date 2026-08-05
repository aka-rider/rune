//! WP1 "Done when" tests: `Msg::Error` posts into the message log and opens
//! the collapsible pane above the footer WITHOUT taking focus (plan WP1
//! decision 3 — non-modal, unlike the pre-WP1 modal banner this replaces);
//! `^E` toggles focus/collapse; `Esc` inside the pane collapses it and
//! returns focus to the editor; the pane's height is capped at the 40% end
//! of the requested band; and a C0 byte in a posted message is sanitized
//! before it can ever reach the rendered grid.
//!
//! WP3: mouse drag-selection inside the pane, with copy-to-clipboard on
//! button release, routed through the same capped OSC-52 path every other
//! copy in the app uses.
//!
//! Replaces `tests/banner.rs` (plan WP1: the old modal error banner/`banner.rs` no
//! longer exist — errors are message-log entries now, and the Guard's own
//! priority tests moved to `guard.rs`'s own unit tests, since `set_guard`'s
//! "never displace" rule no longer needs a second modal variant to compare
//! against).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use ratatui::layout::Rect;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::clipboard::{OSC52_MAX_PAYLOAD_BYTES, osc52_copy};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::layout;
use rune_tui::messages;
use rune_tui::pane::Pane;
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_for(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.frame_width = WIDTH;
    app.frame_height = HEIGHT;
    app.sync_view();
    app
}

fn frame_text(app: &App) -> String {
    testgrid::grid(app, WIDTH, HEIGHT).concat()
}

fn key(code: KeyCode) -> Msg {
    Msg::Key(KeyInput {
        code,
        mods: Mods::NONE,
    })
}

fn ctrl_e() -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char('e'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

/// `⌘C` — one of the pane's own two copy chords (plan WP3.S5), the exact
/// chord the editor's own `Copy` row binds too.
fn super_c() -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char('c'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    })
}

/// The pane's own `Rect` this frame — panics (test-only) if it's closed,
/// since every WP3 test opens it first.
fn pane_rect(app: &App) -> Rect {
    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    layout::geometry(area, app)
        .messages
        .expect("test setup: the pane must be open")
}

/// Sends one raw mouse event through the real `update`, returning the
/// `Effects` it produced — mirrors `splitter_drag.rs`'s own `send` helper,
/// except this one hands back `Effects` (WP3 tests assert on `effects.raw`,
/// the OSC-52 clipboard write) instead of resyncing for a later geometry
/// read.
fn mouse(app: &mut App, kind: MouseKind, column: u16, row: u16) -> Effects {
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Mouse(MouseInput {
            kind,
            column,
            row,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
    effects
}

/// `Msg::Error` posts into the log and opens the pane (plan WP1) rather
/// than raising the pre-WP1 modal banner — the routing chokepoint
/// (`dispatch::update_inner`'s `Msg::Error` arm -> `messages::error`).
#[test]
fn an_error_posts_and_opens_the_pane() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Error("boom".to_string()), &mut effects);

    assert!(messages::is_open(&app));
    assert_eq!(messages::newest_text(&app), Some("boom"));

    app.sync_view();
    let text = frame_text(&app);
    assert!(
        text.contains("boom"),
        "expected the error text somewhere in the frame"
    );
}

/// A posted message never takes focus (plan WP1 decision 3) — the editor
/// keeps it, and a subsequent printable key still reaches the buffer.
#[test]
fn the_editor_keeps_focus_and_a_character_still_reaches_the_buffer() {
    let mut app = app_for("hello");
    let id = app.active;
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Error("boom".to_string()), &mut effects);

    assert_eq!(
        app.focus(),
        Pane::Editor,
        "posting a message must never steal focus"
    );

    let mut effects2 = Effects::default();
    app::update(&mut app, key(KeyCode::Char('x')), &mut effects2);

    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "xhello",
        "a character typed after a message posts must still reach the buffer"
    );
}

/// `^E` on a closed pane opens and focuses it; a second `^E` (now focused)
/// collapses it and returns focus to the editor.
#[test]
fn ctrl_e_focuses_the_pane_and_a_second_collapses_it() {
    let mut app = app_for("hello");

    let mut effects = Effects::default();
    app::update(&mut app, ctrl_e(), &mut effects);
    assert!(messages::is_open(&app));
    assert_eq!(app.focus(), Pane::Messages);

    let mut effects2 = Effects::default();
    app::update(&mut app, ctrl_e(), &mut effects2);
    assert!(!messages::is_open(&app));
    assert_eq!(app.focus(), Pane::Editor);
}

/// `Esc` inside the pane collapses it and returns focus to the editor —
/// same outcome as the second `^E`, reached a different way.
#[test]
fn escape_in_the_pane_collapses_and_returns_focus_to_the_editor() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(&mut app, ctrl_e(), &mut effects);
    assert_eq!(app.focus(), Pane::Messages);

    let mut effects2 = Effects::default();
    app::update(&mut app, key(KeyCode::Escape), &mut effects2);

    assert!(!messages::is_open(&app));
    assert_eq!(app.focus(), Pane::Editor);
}

/// `^E` on an empty log opens the pane showing the `EMPTY_TEXT` placeholder.
#[test]
fn ctrl_e_on_an_empty_log_shows_no_messages() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(&mut app, ctrl_e(), &mut effects);
    app.sync_view();

    let text = frame_text(&app);
    assert!(
        text.contains("no messages"),
        "expected the empty-log placeholder somewhere in the frame, got {text:?}"
    );
}

/// The pane's rendered height never exceeds the 40% end of the requested
/// 30-40% band, however much text the log holds.
#[test]
fn pane_height_never_exceeds_forty_percent_of_the_frame() {
    let mut app = app_for("hello");
    let huge: String = (0..80)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    messages::error(&mut app, huge);
    app.sync_view();

    let cap = (app.frame_height as usize * 2 / 5) as u16 + 1;
    assert!(
        messages::height(&app, app.frame_height) <= cap,
        "pane height {} exceeds the 40% cap {}",
        messages::height(&app, app.frame_height),
        cap
    );
}

/// A C0 control byte in a posted message is sanitized before it can ever
/// reach the log's own document — and therefore never reaches the rendered
/// grid.
#[test]
fn a_c0_byte_in_a_posted_message_never_reaches_the_rendered_grid() {
    let mut app = app_for("hello");
    messages::error(&mut app, "bad\u{0}\u{7}text");

    assert_eq!(
        messages::newest_text(&app),
        Some("badtext"),
        "C0 control bytes must be stripped before the entry is stored"
    );

    app.sync_view();
    let text = frame_text(&app);
    assert!(text.contains("badtext"));
}

/// Opening the pane shrinks the editor's viewport (plan Gotchas), but the
/// caret must still land inside whatever viewport remains — `Document::
/// sync`'s own scroll-to-cursor reconciliation, re-run every `sync_view`.
#[test]
fn opening_the_pane_does_not_scroll_the_caret_out_of_view() {
    let content: String = (0..60)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut app = app_for(&content);
    let end = app.active_doc().buffer.len();
    app.active_doc_mut().cursors = CursorSet::new(end);
    app.sync_view();

    let mut effects = Effects::default();
    app::update(&mut app, ctrl_e(), &mut effects);
    app.sync_view();

    let doc = app.active_doc();
    let view = doc.view.as_ref().expect("synced view");
    let buffer_point = doc
        .buffer
        .offset_to_line_col(doc.cursors.primary().position);
    let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
    let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
    let display_row = view.display.wrap_to_display(wrap_point.row);

    assert!(
        display_row >= doc.viewport.scroll_row
            && display_row < doc.viewport.scroll_row + doc.viewport.height as usize,
        "caret row {display_row} is outside the post-open viewport \
         [{}, {})",
        doc.viewport.scroll_row,
        doc.viewport.scroll_row + doc.viewport.height as usize
    );
}

/// A `Warn` glyph (`"! "`) is pure ASCII, unlike `Info`'s middle dot — so a
/// warning's rendered row has a exactly-one-byte-per-cell mapping from the
/// glyph prefix all the way through the message text, and a drag's target
/// columns can be reasoned about directly instead of guessing at a
/// multi-byte glyph's cell width.
const WARN_PREFIX_COLS: u16 = 2; // "! "

/// WP3.S6: a plain drag inside the pane selects text and, on release,
/// copies exactly the dragged range through the same capped OSC-52 path
/// every other copy in the app uses.
#[test]
fn dragging_inside_the_pane_selects_and_copies_on_release() {
    let mut app = app_for("hello");
    messages::warn(&mut app, "hello world");
    app.sync_view();

    let rect = pane_rect(&app);
    let row = rect.y + 1; // first content row, past the separator
    let start_col = rect.x + WARN_PREFIX_COLS; // "hello" starts here
    let end_col = rect.x + 40; // past the row's content — clamps to its end

    mouse(&mut app, MouseKind::Down(MouseButton::Left), start_col, row);
    mouse(&mut app, MouseKind::Drag(MouseButton::Left), end_col, row);
    let effects = mouse(&mut app, MouseKind::Up(MouseButton::Left), end_col, row);

    assert_eq!(
        effects.raw,
        vec![osc52_copy(b"hello world")],
        "release must copy exactly the dragged selection"
    );
}

/// WP3.S6 + the `mouse.rs` bug fix this plan describes: a drag begun in the
/// pane must still copy — and must clear its latch — even when the button
/// comes up somewhere else entirely (mode 1002 reports no hover, so a lost
/// release has no second signal to recover from).
#[test]
fn a_drag_released_outside_the_pane_still_clears_the_drag_and_still_copies() {
    let mut app = app_for("hello");
    messages::warn(&mut app, "hello world");
    app.sync_view();

    let rect = pane_rect(&app);
    let row = rect.y + 1;
    let start_col = rect.x + WARN_PREFIX_COLS;
    let end_col = rect.x + 40;

    mouse(&mut app, MouseKind::Down(MouseButton::Left), start_col, row);
    mouse(&mut app, MouseKind::Drag(MouseButton::Left), end_col, row);
    // Released far above the pane — outside every rect in the frame.
    let effects = mouse(&mut app, MouseKind::Up(MouseButton::Left), 0, 0);

    assert_eq!(
        effects.raw,
        vec![osc52_copy(b"hello world")],
        "a release outside the pane must still copy the drag's selection"
    );

    // The latch must be cleared too: a second, unrelated Up must not copy
    // again.
    let effects_again = mouse(&mut app, MouseKind::Up(MouseButton::Left), 0, 0);
    assert!(
        effects_again.raw.is_empty(),
        "the drag must be cleared after its first release"
    );
}

/// WP3.S6: a drag inside the EDITOR must never reach the log document's own
/// cursor — proven by focusing the pane afterward and asking it to copy,
/// which must find no selection to copy at all.
#[test]
fn a_drag_in_the_editor_never_touches_the_log_documents_cursor() {
    let mut app = app_for("hello world\n");
    messages::warn(&mut app, "hello world");
    app.sync_view();

    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    let editor = layout::geometry(area, &app).editor;
    mouse(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        editor.x,
        editor.y,
    );
    mouse(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        editor.x + 5,
        editor.y,
    );
    mouse(
        &mut app,
        MouseKind::Up(MouseButton::Left),
        editor.x + 5,
        editor.y,
    );

    let mut effects = Effects::default();
    app::update(&mut app, ctrl_e(), &mut effects); // focus the pane
    let mut copy_effects = Effects::default();
    app::update(&mut app, super_c(), &mut copy_effects);

    assert!(
        copy_effects.raw.is_empty(),
        "an editor drag must never leave a selection on the log document"
    );
}

/// WP3.S6: a click inside the pane must never move the active editor
/// document's own caret.
#[test]
fn a_click_in_the_pane_does_not_move_the_editor_caret() {
    let mut app = app_for("hello world\n");
    messages::warn(&mut app, "hello world");
    app.sync_view();
    let before = app.active_doc().cursors.primary().position;

    let rect = pane_rect(&app);
    mouse(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        rect.x + WARN_PREFIX_COLS,
        rect.y + 1,
    );

    assert_eq!(
        app.active_doc().cursors.primary().position,
        before,
        "a click inside the pane must never move the active editor document's caret"
    );
}

/// WP3.S6 (`rune-tui C 6` parity): a selection over `OSC52_MAX_PAYLOAD_
/// BYTES` must post an error message instead of writing a raw sequence a
/// terminal multiplexer would just drop — triple-click selects the whole
/// (wrapped) paragraph in one gesture, mirroring the editor's own
/// whole-logical-line triple-click.
#[test]
fn an_over_cap_selection_posts_an_error_instead_of_writing_raw() {
    let mut app = app_for("hello");
    let huge = "x".repeat(OSC52_MAX_PAYLOAD_BYTES + 1);
    messages::warn(&mut app, huge);
    app.sync_view();

    let rect = pane_rect(&app);
    let row = rect.y + 1;
    let col = rect.x + WARN_PREFIX_COLS;

    mouse(&mut app, MouseKind::Down(MouseButton::Left), col, row);
    mouse(&mut app, MouseKind::Down(MouseButton::Left), col, row);
    mouse(&mut app, MouseKind::Down(MouseButton::Left), col, row); // triple click
    let release = mouse(&mut app, MouseKind::Up(MouseButton::Left), col, row);

    assert!(
        release.raw.is_empty(),
        "an over-cap selection must never reach the OSC-52 raw output"
    );
    assert!(
        messages::newest_text(&app).is_some_and(|t| t.contains("too large")),
        "an over-cap copy must post a message reporting the failure"
    );
}
