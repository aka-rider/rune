//! WP1 "Done when" tests: `Msg::Error` posts into the message log and opens
//! the collapsible pane above the footer WITHOUT taking focus (plan WP1
//! decision 3 — non-modal, unlike the pre-WP1 modal banner this replaces);
//! `^E` toggles focus/collapse; `Esc` inside the pane collapses it and
//! returns focus to the editor; the pane's height is capped at the 40% end
//! of the requested band; and a C0 byte in a posted message is sanitized
//! before it can ever reach the rendered grid.
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

use rune_core::buffer::Buffer;
use rune_core::cursor::{Cursor, CursorSet};
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::messages;
use rune_tui::pane::Pane;
use rune_tui::runtime::{CmdKind, Effects, Msg};
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

/// An `Info` post arms exactly one `MessagesCollapseTimeout` `Cmd`, and
/// sending the matching `Msg` back collapses the pane (plan WP2.S5).
#[test]
fn an_info_post_arms_exactly_one_timeout_cmd_and_the_matching_msg_collapses_the_pane() {
    let mut app = app_for("hello");
    messages::info(&mut app, "saved");
    assert!(messages::is_open(&app));

    let mut effects = Effects::default();
    app::update(&mut app, key(KeyCode::Right), &mut effects);
    let armed: Vec<_> = effects
        .cmds
        .iter()
        .filter(|c| c.kind() == CmdKind::MessagesCollapseTimeout)
        .collect();
    assert_eq!(
        armed.len(),
        1,
        "expected exactly one auto-collapse timer armed"
    );

    let mut effects2 = Effects::default();
    app::update(
        &mut app,
        Msg::MessagesCollapseTimeout { generation: 0 },
        &mut effects2,
    );
    assert!(
        !messages::is_open(&app),
        "the matching generation must collapse the pane"
    );
}

/// A stale generation (superseded by a second post before the first timer
/// fired) is ignored; only the current generation collapses the pane.
#[test]
fn a_stale_generation_is_ignored_and_a_fresh_one_supersedes_it() {
    let mut app = app_for("hello");
    messages::info(&mut app, "first");
    let mut e0 = Effects::default();
    app::update(&mut app, key(KeyCode::Right), &mut e0); // arms generation 0

    messages::info(&mut app, "second"); // clears armed, restarting the countdown
    let mut e1 = Effects::default();
    app::update(&mut app, key(KeyCode::Right), &mut e1); // arms generation 1

    let mut stale_effects = Effects::default();
    app::update(
        &mut app,
        Msg::MessagesCollapseTimeout { generation: 0 },
        &mut stale_effects,
    );
    assert!(
        messages::is_open(&app),
        "a stale (generation 0) timeout must not collapse the pane"
    );

    let mut fresh_effects = Effects::default();
    app::update(
        &mut app,
        Msg::MessagesCollapseTimeout { generation: 1 },
        &mut fresh_effects,
    );
    assert!(
        !messages::is_open(&app),
        "the current (generation 1) timeout must collapse the pane"
    );
}

/// An `Error` post never arms an auto-collapse timer (CONSTITUTION §0.1: a
/// data-risk message must stay visible until dismissed) — the pane stays
/// open with no timer pending.
#[test]
fn an_error_post_arms_nothing_and_the_pane_stays_open() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Error("boom".to_string()), &mut effects);

    let armed = effects
        .cmds
        .iter()
        .any(|c| c.kind() == CmdKind::MessagesCollapseTimeout);
    assert!(
        !armed,
        "an error post must never arm the auto-collapse timer"
    );
    assert!(messages::is_open(&app));
}

/// A focused pane arms nothing — the user is actively reading/scrolling it.
#[test]
fn a_focused_pane_arms_nothing() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(&mut app, ctrl_e(), &mut effects);
    assert_eq!(app.focus(), Pane::Messages);

    let armed = effects
        .cmds
        .iter()
        .any(|c| c.kind() == CmdKind::MessagesCollapseTimeout);
    assert!(
        !armed,
        "a focused pane must never arm the auto-collapse timer"
    );
}

/// A pane whose log document carries a non-empty selection arms nothing —
/// collapsing out from under a selection the user is about to copy would
/// discard it.
#[test]
fn a_pane_with_a_selection_arms_nothing() {
    let mut app = app_for("hello");
    messages::info(&mut app, "saved");
    messages::doc_mut(&mut app).cursors = CursorSet::new_from(&[Cursor {
        position: 0,
        anchor: 3,
        desired_col: 0,
        id: 1,
    }]);

    let mut effects = Effects::default();
    app::update(&mut app, key(KeyCode::Right), &mut effects);
    let armed = effects
        .cmds
        .iter()
        .any(|c| c.kind() == CmdKind::MessagesCollapseTimeout);
    assert!(
        !armed,
        "a pane with a selection on its log document must never arm the auto-collapse timer"
    );
}
