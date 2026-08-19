//! "Done when" tests: `Msg::Error` posts into the message log and opens
//! the collapsible pane above the footer WITHOUT taking focus — non-modal,
//! unlike the old modal banner this replaces; `^E` toggles focus/collapse;
//! `Esc` inside the pane collapses it and returns focus to the editor; the
//! pane's height is capped at the 40% end of the requested band; and a C0
//! byte in a posted message is sanitized before it can ever reach the
//! rendered grid.
//!
//! Copy coverage — the pane's own copy chord, and mouse drag-selection with
//! copy-to-clipboard on button release — lives in the sibling
//! `messages_mouse.rs`.
//!
//! Replaces `tests/banner.rs`: the old modal error banner/`banner.rs` no
//! longer exist — errors are message-log entries now, and the Guard's own
//! priority tests moved to `guard.rs`'s own unit tests, since `set_guard`'s
//! "never displace" rule no longer needs a second modal variant to compare
//! against.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod messages_common;

use rune_core::coords::WrapRow;
use rune_core::cursor::{Cursor, CursorId, CursorSet};
use rune_tui::app;
use rune_tui::keymap::KeyCode;
use rune_tui::messages;
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg, TimerKey};

use messages_common::{app_for, ctrl_e, frame_text, key};

/// `Msg::Error` posts into the log and opens the pane rather than raising
/// the old modal banner — the routing chokepoint (`dispatch::update_inner`'s
/// `Msg::Error` arm -> `messages::error`).
#[test]
fn an_error_posts_and_opens_the_pane() {
    let mut session = app_for("hello");
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Error("boom".to_string()),
        &mut effects,
    );

    assert!(messages::is_open(session.app()));
    assert_eq!(messages::newest_text(session.app()), Some("boom"));

    session.app_mut().sync_view();
    let text = frame_text(&mut session);
    assert!(
        text.contains("boom"),
        "expected the error text somewhere in the frame"
    );
}

/// A posted message never takes focus — the editor keeps it, and a
/// subsequent printable key still reaches the buffer.
#[test]
fn the_editor_keeps_focus_and_a_character_still_reaches_the_buffer() {
    let mut session = app_for("hello");
    let id = session.app().active;
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Error("boom".to_string()),
        &mut effects,
    );

    assert_eq!(
        session.app().focus(),
        Pane::Editor,
        "posting a message must never steal focus"
    );

    let mut effects2 = Effects::default();
    app::update(session.app_mut(), key(KeyCode::Char('x')), &mut effects2);

    assert_eq!(
        session.app().doc(id).unwrap().buffer.content(),
        "xhello",
        "a character typed after a message posts must still reach the buffer"
    );
}

/// `^E` on a closed pane opens and focuses it; a second `^E` (now focused)
/// collapses it and returns focus to the editor.
#[test]
fn ctrl_e_focuses_the_pane_and_a_second_collapses_it() {
    let mut session = app_for("hello");

    let mut effects = Effects::default();
    app::update(session.app_mut(), ctrl_e(), &mut effects);
    assert!(messages::is_open(session.app()));
    assert_eq!(session.app().focus(), Pane::Messages);

    let mut effects2 = Effects::default();
    app::update(session.app_mut(), ctrl_e(), &mut effects2);
    assert!(!messages::is_open(session.app()));
    assert_eq!(session.app().focus(), Pane::Editor);
}

/// `Esc` inside the pane collapses it and returns focus to the editor —
/// same outcome as the second `^E`, reached a different way.
#[test]
fn escape_in_the_pane_collapses_and_returns_focus_to_the_editor() {
    let mut session = app_for("hello");
    let mut effects = Effects::default();
    app::update(session.app_mut(), ctrl_e(), &mut effects);
    assert_eq!(session.app().focus(), Pane::Messages);

    let mut effects2 = Effects::default();
    app::update(session.app_mut(), key(KeyCode::Escape), &mut effects2);

    assert!(!messages::is_open(session.app()));
    assert_eq!(session.app().focus(), Pane::Editor);
}

/// `^E` on an empty log opens the pane showing the `EMPTY_TEXT` placeholder.
#[test]
fn ctrl_e_on_an_empty_log_shows_no_messages() {
    let mut session = app_for("hello");
    let mut effects = Effects::default();
    app::update(session.app_mut(), ctrl_e(), &mut effects);
    session.app_mut().sync_view();

    let text = frame_text(&mut session);
    assert!(
        text.contains("no messages"),
        "expected the empty-log placeholder somewhere in the frame, got {text:?}"
    );
}

/// The pane's rendered height never exceeds the 40% end of the requested
/// 30-40% band, however much text the log holds.
#[test]
fn pane_height_never_exceeds_forty_percent_of_the_frame() {
    let mut session = app_for("hello");
    let huge: String = (0..80)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    messages::error(session.app_mut(), huge);
    session.app_mut().sync_view();

    let frame_height = session.app().frame_height;
    let cap = (frame_height as usize * 2 / 5) as u16 + 1;
    assert!(
        messages::height(session.app(), frame_height) <= cap,
        "pane height {} exceeds the 40% cap {}",
        messages::height(session.app(), frame_height),
        cap
    );
}

/// A C0 control byte, and a C1 control character (`U+0080`-`U+009F`, finding
/// 4 — unlike C0/DEL, these decode from UTF-8 as ordinary multi-byte
/// sequences, not raw control bytes), are both sanitized before a posted
/// message can ever reach the log's own document — and therefore never
/// reach the rendered grid.
#[test]
fn a_c0_byte_in_a_posted_message_never_reaches_the_rendered_grid() {
    let mut session = app_for("hello");
    messages::error(session.app_mut(), "bad\u{0}\u{7}\u{85}text");

    assert_eq!(
        messages::newest_text(session.app()),
        Some("badtext"),
        "C0 and C1 control characters must be stripped before the entry is stored"
    );

    session.app_mut().sync_view();
    let text = frame_text(&mut session);
    assert!(text.contains("badtext"));
}

/// Opening the pane shrinks the editor's viewport, but the caret must still
/// land inside whatever viewport remains — `Document::sync`'s own
/// scroll-to-cursor reconciliation, re-run every `sync_view`.
#[test]
fn opening_the_pane_does_not_scroll_the_caret_out_of_view() {
    let content: String = (0..60)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut session = app_for(&content);
    let end = session.app().active_doc().buffer.len();
    session.app_mut().active_doc_mut().cursors = CursorSet::new(end);
    session.app_mut().sync_view();

    let mut effects = Effects::default();
    app::update(session.app_mut(), ctrl_e(), &mut effects);
    session.app_mut().sync_view();

    let app = session.app();
    let doc = app.active_doc();
    let view = doc.view.as_ref().expect("synced view");
    let buffer_point = doc
        .buffer
        .offset_to_line_col(doc.cursors.primary().position);
    let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
    let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
    let display_row = view.display.wrap_to_display(WrapRow(wrap_point.row));

    assert!(
        display_row >= doc.viewport.scroll_row
            && display_row < doc.viewport.scroll_row + doc.viewport.height as usize,
        "caret row {display_row} is outside the post-open viewport \
         [{}, {})",
        doc.viewport.scroll_row,
        doc.viewport.scroll_row + doc.viewport.height as usize
    );
}

/// The pane's height must be a fixed point of the settle step. `relayout`
/// sizes the editor viewport from a rect with the pane's height carved out of
/// it, and that height comes from the log document's synced view — so a
/// settle step that syncs the pane after laying out would leave the editor
/// trailing the pane by one pass, and the rows built for one frame would not
/// fit the rect they are blitted into.
#[test]
fn a_second_settle_after_a_post_changes_neither_the_viewport_nor_the_rows() {
    let mut session = app_for("# Title\n\nsome body text\n");

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Error("something went wrong\nwith a second line".to_string()),
        &mut effects,
    );
    session.app_mut().sync_view();

    let height_after_first = session.app().active_doc().viewport.height;
    let rows_after_first = session.grid(messages_common::WIDTH, messages_common::HEIGHT);

    session.app_mut().sync_view();

    assert_eq!(
        height_after_first,
        session.app().active_doc().viewport.height,
        "the editor viewport height changed on a second settle with no \
         intervening message"
    );
    assert_eq!(
        rows_after_first,
        session.grid(messages_common::WIDTH, messages_common::HEIGHT),
        "the rendered rows changed on a second settle with no intervening \
         message"
    );
}

/// The same fixed point, reached by resizing rather than by posting: the log
/// document re-wraps at the new width, so its row count — and the pane's
/// height — changes with no message involved at all.
#[test]
fn a_second_settle_after_a_resize_with_the_pane_open_is_stable() {
    let mut session = app_for("# Title\n\nsome body text\n");

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Error(
            "a message long enough that it must re-wrap onto a different \
             number of rows when the terminal narrows"
                .to_string(),
        ),
        &mut effects,
    );
    session.app_mut().sync_view();

    app::update(
        session.app_mut(),
        Msg::Resize(40, messages_common::HEIGHT),
        &mut effects,
    );
    session.app_mut().sync_view();

    let height_after_first = session.app().active_doc().viewport.height;
    let rows_after_first = session.grid(40, messages_common::HEIGHT);

    session.app_mut().sync_view();

    assert_eq!(
        height_after_first,
        session.app().active_doc().viewport.height,
        "the editor viewport height changed on a second settle after a resize"
    );
    assert_eq!(
        rows_after_first,
        session.grid(40, messages_common::HEIGHT),
        "the rendered rows changed on a second settle after a resize"
    );
}

/// Finding 1: once enough messages overflow the pane's capped content
/// height, a freshly posted message must still be visible — pinned to the
/// tail, not left off-screen above a top-anchored scroll position the pane
/// never moves.
#[test]
fn a_newest_message_is_visible_after_the_pane_overflows() {
    let mut session = app_for("hello");
    for i in 0..20 {
        messages::error(session.app_mut(), format!("entry number {i}"));
    }
    session.app_mut().sync_view();

    let text = frame_text(&mut session);
    assert!(
        text.contains("entry number 19"),
        "expected the newest entry visible somewhere in the frame, got {text:?}"
    );
}

/// An `Info` post arms exactly one auto-collapse timeout directly on
/// `App::timers` (no `Cmd` any more), and sending the matching `Msg` back
/// collapses the pane.
#[test]
fn an_info_post_arms_exactly_one_timeout_cmd_and_the_matching_msg_collapses_the_pane() {
    let mut session = app_for("hello");
    messages::info(session.app_mut(), "saved");
    assert!(messages::is_open(session.app()));

    let mut effects = Effects::default();
    app::update(session.app_mut(), key(KeyCode::Right), &mut effects);
    assert!(
        messages::is_collapse_armed(session.app()),
        "expected the auto-collapse timer armed"
    );

    let mut effects2 = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Timer {
            key: TimerKey::MessagesCollapse,
            generation: rune_tui::generation::Generation::ZERO,
        },
        &mut effects2,
    );
    assert!(
        !messages::is_open(session.app()),
        "the matching generation must collapse the pane"
    );
}

/// A stale generation (superseded by a second post before the first timer
/// fired) is ignored; only the current generation collapses the pane.
#[test]
fn a_stale_generation_is_ignored_and_a_fresh_one_supersedes_it() {
    let mut session = app_for("hello");
    messages::info(session.app_mut(), "first");
    let mut e0 = Effects::default();
    app::update(session.app_mut(), key(KeyCode::Right), &mut e0); // arms generation 0

    messages::info(session.app_mut(), "second"); // clears armed, restarting the countdown
    let mut e1 = Effects::default();
    app::update(session.app_mut(), key(KeyCode::Right), &mut e1); // arms generation 1

    let mut stale_effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Timer {
            key: TimerKey::MessagesCollapse,
            generation: rune_tui::generation::Generation::ZERO,
        },
        &mut stale_effects,
    );
    assert!(
        messages::is_open(session.app()),
        "a stale (generation 0) timeout must not collapse the pane"
    );

    let mut fresh_effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Timer {
            key: TimerKey::MessagesCollapse,
            generation: rune_tui::generation::Generation::from_raw(1),
        },
        &mut fresh_effects,
    );
    assert!(
        !messages::is_open(session.app()),
        "the current (generation 1) timeout must collapse the pane"
    );
}

/// An `Error` post never arms an auto-collapse timer — a data-risk message
/// must stay visible until dismissed — the pane stays open with no timer
/// pending.
#[test]
fn an_error_post_arms_nothing_and_the_pane_stays_open() {
    let mut session = app_for("hello");
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Error("boom".to_string()),
        &mut effects,
    );

    assert!(
        !messages::is_collapse_armed(session.app()),
        "an error post must never arm the auto-collapse timer"
    );
    assert!(messages::is_open(session.app()));
}

/// A focused pane arms nothing — the user is actively reading/scrolling it.
#[test]
fn a_focused_pane_arms_nothing() {
    let mut session = app_for("hello");
    let mut effects = Effects::default();
    app::update(session.app_mut(), ctrl_e(), &mut effects);
    assert_eq!(session.app().focus(), Pane::Messages);

    assert!(
        !messages::is_collapse_armed(session.app()),
        "a focused pane must never arm the auto-collapse timer"
    );
}

/// A pane whose log document carries a non-empty selection arms nothing —
/// collapsing out from under a selection the user is about to copy would
/// discard it.
#[test]
fn a_pane_with_a_selection_arms_nothing() {
    let mut session = app_for("hello");
    messages::info(session.app_mut(), "saved");
    messages::doc_mut(session.app_mut()).cursors = CursorSet::new_from(&[Cursor {
        position: 0,
        anchor: 3,
        desired_col: 0,
        id: CursorId::FIRST,
    }]);

    let mut effects = Effects::default();
    app::update(session.app_mut(), key(KeyCode::Right), &mut effects);
    assert!(
        !messages::is_collapse_armed(session.app()),
        "a pane with a selection on its log document must never arm the auto-collapse timer"
    );
}
