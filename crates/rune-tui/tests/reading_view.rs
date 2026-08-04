//! WP7 "Done when" tests for the reading-view plan
//! (`in-read-only-mode-markdown-serialized-spark.md`) — this file is what
//! proves the originally reported bug is fixed: *"In read-only mode,
//! markdown reading, there's no cursor but markdown elements still behave
//! as if they are editable. For instance, markdown table is not rendered
//! as a table but as an `|column1|column2|...`"*.
//!
//! Drives real keystrokes through `app::update` (the `tests/help.rs`/
//! `rename_common` idiom), not `commands::edit::*` directly
//! (`tests/edit_commands.rs`'s idiom, wrong shape for this file — it never
//! builds a `KeyInput` or calls `app::update` at all). Render assertions
//! reuse `tests/tui_render_common/mod.rs` rather than adding a fourth local
//! copy of its helpers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::document::ReadOnly;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

mod tui_render_common;
use tui_render_common::{HEIGHT, WIDTH, app_for, caret_column, full_text, render_to_test_backend};

fn app_basic(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn key(code: KeyCode, mods: Mods) -> Msg {
    Msg::Key(KeyInput { code, mods })
}

fn plain(code: KeyCode) -> Msg {
    key(code, Mods::NONE)
}

fn ctrl(c: char) -> Msg {
    key(
        KeyCode::Char(c),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    )
}

fn send(app: &mut App, msg: Msg) {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
}

/// THE REGRESSION TEST. F1 opens the Help tab; the `## Global` table is the
/// first table in `help_markdown()` (source line 5, `| Key | Action |`), so
/// it sits on screen at `scroll_row == 0` with no scrolling needed. A few
/// `Down` presses land the cursor inside the table's own lines. Before
/// WP1, the root reveal grant keyed off `focused` alone, so a focused
/// read-only document (Help always is) still revealed raw markdown under
/// its invisible cursor — this is the exact bug report: box borders
/// replaced by a literal `| Key | Action |` source row.
#[test]
fn help_tab_table_stays_boxed_when_the_cursor_lands_inside_it() {
    let mut app = app_basic("hello");

    send(&mut app, plain(KeyCode::F1));
    assert_eq!(
        app.active_doc().read_only,
        ReadOnly::Always,
        "F1 must land on the read-only Help document"
    );

    // The freshly minted Help document has its own, not-yet-sized
    // viewport; size it the same way `app_basic` sized the original.
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);

    // "# Help" / "" / "## Global" / "" / "| Key | Action |" / "| --- | --- |"
    // / first data row: six `Down` presses land the cursor on a real data
    // row, well inside the table's own line range.
    for _ in 0..6 {
        send(&mut app, plain(KeyCode::Down));
    }
    app.sync_view();

    let buf = render_to_test_backend(&app);
    let text = full_text(&buf, HEIGHT, WIDTH);

    assert!(
        text.contains('│'),
        "expected box-drawing borders around the Help table in reading view:\n{text}"
    );
    assert!(
        !text.contains("| Key | Action |"),
        "the raw markdown table source leaked onto the screen instead of a rendered box:\n{text}"
    );
}

/// An ordinary (non-`Always`) document with a table, cursor inside it:
/// `⌃P` boxes the table (root grant `Never`, cursor position irrelevant);
/// `⌃P` again returns to `ReadOnly::No`, where the cursor still sitting
/// inside the table's lines reveals its raw source again — pinning BOTH
/// directions of the toggle.
#[test]
fn ctrl_p_toggles_an_ordinary_documents_table_between_boxed_and_raw() {
    let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n\ntail\n";
    let cursor = content.find("Alice").expect("fixture has a data row");
    let mut app = app_for(content, cursor, true);

    send(&mut app, ctrl('p'));
    assert_eq!(app.active_doc().read_only, ReadOnly::Reading);
    app.sync_view();

    let boxed = full_text(&render_to_test_backend(&app), HEIGHT, WIDTH);
    assert!(
        boxed.contains('│'),
        "reading view must box the table regardless of cursor position:\n{boxed}"
    );
    assert!(
        !boxed.contains("| Alice | 30 |"),
        "reading view must not reveal the raw source row:\n{boxed}"
    );

    send(&mut app, ctrl('p'));
    assert_eq!(app.active_doc().read_only, ReadOnly::No);
    app.sync_view();

    let raw = full_text(&render_to_test_backend(&app), HEIGHT, WIDTH);
    assert!(
        raw.contains("| Alice | 30 |"),
        "leaving reading view with the cursor still inside the table must reveal raw source again:\n{raw}"
    );
}

/// `⌃P` on the Help tab is a no-op: `ReadOnly::Always` has no editable form
/// to toggle back to, so `commands::reading::toggle` refuses and posts its
/// status message instead of flipping state.
#[test]
fn ctrl_p_on_the_help_tab_refuses_and_posts_a_status_message() {
    let mut app = app_basic("hello");
    send(&mut app, plain(KeyCode::F1));
    assert_eq!(app.active_doc().read_only, ReadOnly::Always);

    send(&mut app, ctrl('p'));

    assert_eq!(
        app.active_doc().read_only,
        ReadOnly::Always,
        "⌃P must not change the Help document's ReadOnly state"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some(ReadOnly::Always.refusal_message())
    );
}

/// In reading view, a printable key must not mutate the buffer or mark it
/// dirty — the mutation chokepoint (`commands::edit_core`) refuses any
/// `is_read_only()` document, and `Reading` is one. (Undo/redo blocking is
/// already covered by `reading_view_blocks_undo_and_redo`; not duplicated
/// here.)
#[test]
fn a_printable_key_in_reading_view_never_mutates_the_buffer() {
    let mut app = app_basic("hello world");
    app.active_doc_mut().read_only = ReadOnly::Reading;
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, plain(KeyCode::Char('x')));

    assert_eq!(
        app.active_doc().buffer.content(),
        before,
        "a printable key must not mutate a reading-view document"
    );
    assert!(
        !app.active_doc().is_dirty(),
        "a rejected edit must never mark a reading-view document dirty"
    );
}

/// `⌃R` (rename) in reading view refuses with the `Reading` wording
/// ("reading view — ⌃P to edit"), not the `Always` wording ("this document
/// is read-only") — the two chokepoints (`app.rs::focus_title`,
/// `rename::begin`'s `Commit::Refused`) derive the string from the
/// `ReadOnly` variant, and a mechanical widening to a blanket
/// `is_read_only()` would have collapsed them onto the same message.
#[test]
fn ctrl_r_in_reading_view_refuses_with_the_reading_wording_not_the_always_wording() {
    let mut app = app_basic("hello");
    app.active_doc_mut().read_only = ReadOnly::Reading;

    send(&mut app, ctrl('r'));

    assert_eq!(
        app.status_message.as_deref(),
        Some(ReadOnly::Reading.refusal_message())
    );
    assert_ne!(
        app.status_message.as_deref(),
        Some(ReadOnly::Always.refusal_message()),
        "a Reading document must not get the Always refusal wording"
    );
}

/// No caret is painted anywhere on screen in reading view: `has_insertion_
/// point()` is `focused && !is_read_only()`, and `Reading` is read-only, so
/// the caret gate (the same predicate the reveal gate now shares) suppresses
/// it everywhere, not just over the elements it conceals.
#[test]
fn reading_view_paints_no_caret_anywhere() {
    let content = "# Doc\n\nSome paragraph text with **bold** and a [link](x).\n";
    let cursor = content.find("bold").expect("fixture has bold text");
    let mut app = app_for(content, cursor, true);
    app.active_doc_mut().read_only = ReadOnly::Reading;
    app.sync_view();

    let buf = render_to_test_backend(&app);
    for y in 0..HEIGHT {
        assert_eq!(
            caret_column(&buf, y, WIDTH),
            None,
            "row {y} painted a caret while in reading view"
        );
    }
}
