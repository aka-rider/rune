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
// `clippy::panic` joins the other three because this file now pulls in
// `rename_common` (`mod rename_common;` below) — the same shared Rename
// fixture module every one of its other seven consumers already carries
// this allow for, since its store-backed constructors panic on an
// unexpected `DbEvent` ack.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::document::ReadOnly;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

mod tui_render_common;
use tui_render_common::{HEIGHT, WIDTH, app_for, caret_column, full_text, render_to_test_backend};

// Draws the rename fixtures (`app_with`/`seeded_vfs`/`type_new_name`) from
// the Rename suite's own shared setup rather than a second local copy —
// `tests/rename_common/mod.rs`'s own doc comment names every consumer this
// joins.
mod rename_common;

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

/// A ⌘-chorded character — `Command::Undo`'s own modifier
/// (`keymap::editor_bindings::editing::SUP`).
fn sup(c: char) -> Msg {
    key(
        KeyCode::Char(c),
        Mods {
            sup: true,
            ..Mods::NONE
        },
    )
}

/// A ⌘⇧-chorded character — `Command::Redo`'s own modifier
/// (`keymap::editor_bindings::editing::SUP_SHIFT`).
fn sup_shift(c: char) -> Msg {
    key(
        KeyCode::Char(c),
        Mods {
            sup: true,
            shift: true,
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
/// it sits on screen at `scroll_row == 0` with no scrolling needed. Before
/// WP1, the root reveal grant keyed off `focused` alone, so a focused
/// read-only document (Help always is) still revealed raw markdown under
/// its invisible cursor — this is the exact bug report: box borders
/// replaced by a literal `| Key | Action |` source row.
///
/// WP-A re-keyed every motion key in a read-only document to a viewport
/// command (`commands::reading_nav`), so `Down` no longer moves the cursor
/// here at all — driving it with keystrokes the way this test used to would
/// exercise scrolling, not reveal. The capability this test actually
/// covers (a read-only document never reveals raw markdown under its
/// cursor) needs the cursor placed directly instead, exactly as `sync.rs`'s
/// own tests already poke `viewport` directly.
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

    // Land the cursor on a real data row of the `## Global` table, just
    // past its `| --- | --- |` separator.
    let content = app.active_doc().buffer.content().to_string();
    let separator = "| --- | --- |\n";
    let data_row_start = content
        .find(separator)
        .map(|i| i + separator.len())
        .expect("the Global table has a separator row");
    app.active_doc_mut().cursors = CursorSet::new(data_row_start + 2);
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
        ReadOnly::Always.refusal_message()
    );
}

/// In reading view, a printable key must not mutate the buffer or mark it
/// dirty — the mutation chokepoint (`commands::edit_core`) refuses any
/// `is_read_only()` document, and `Reading` is one. (Undo/redo blocking via
/// direct calls is covered by `edit_commands.rs`'s
/// `reading_view_blocks_undo_and_redo`; the keystroke-driven equivalent
/// below, `ctrl_z_and_ctrl_shift_z_are_blocked_in_reading_view`, is this
/// file's own coverage of the same guard through the real resolver.)
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
        ReadOnly::Reading.refusal_message()
    );
    assert_ne!(
        app.status_message.as_deref(),
        ReadOnly::Always.refusal_message(),
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

/// Regression test. `pane::handle_global_command` hoists a `blur_title`
/// commit attempt before EVERY global command's own match arm, `⌃P`
/// included — so a title-focused `⌃P` already runs `rename::begin` once,
/// before `commands::reading::toggle` ever executes, as part of that
/// hoisted gate. `save_in_flight` is the vehicle that keeps `begin`
/// refusing across both this test's presses, for the SAME reason each
/// time: with no guard in `toggle` itself, a `Refused` blur (focus left
/// stuck on the title, `begin`'s own doc comment) did not stop `toggle`
/// from flipping the document `ReadOnly::Reading` right after, because
/// `toggle` never looked at focus. The SECOND `begin` call — triggered by
/// the following `Enter`, on the still-focused title — then hit `doc.
/// is_read_only()` (checked before `save_in_flight`) and refused with the
/// READING wording instead of the save-in-flight one: a manufactured
/// refusal reason from a keystroke the user never aimed at the title at
/// all, and the typed name still trapped behind it. Pins that `⌃P` from
/// the title is now inert: the document stays `ReadOnly::No`, and the
/// refusal reason — and the typed name — survive `⌃P` untouched.
#[test]
fn ctrl_p_while_the_title_holds_focus_does_not_derail_an_in_progress_rename() {
    let mem = rename_common::seeded_vfs();
    let mut app = rename_common::app_with(&mem);
    app.active_doc_mut().save_in_flight = true;

    rename_common::type_new_name(&mut app, "b");
    assert_eq!(
        app.focus(),
        Pane::Title,
        "the title must hold focus mid-rename"
    );
    assert_eq!(app.title.text(), "b.md");

    send(&mut app, ctrl('p'));
    assert_eq!(
        app.focus(),
        Pane::Title,
        "⌃P's own hoisted blur must already have refused (save in flight), leaving focus put"
    );
    assert_eq!(
        app.active_doc().read_only,
        ReadOnly::No,
        "⌃P must not flip the document read-only out from under a blur that just refused"
    );
    assert_eq!(
        app.title.text(),
        "b.md",
        "the typed name must survive ⌃P untouched"
    );

    send(&mut app, plain(KeyCode::Enter));

    assert_eq!(
        app.focus(),
        Pane::Title,
        "still refused (save in flight), same as before ⌃P ever fired"
    );
    assert_eq!(
        app.title.text(),
        "b.md",
        "the typed name must still not be discarded"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("can't rename while a save is in flight"),
        "the refusal reason must stay the save-in-flight one, not flip to a \
         read-only refusal ⌃P would otherwise have manufactured"
    );
    assert_ne!(
        app.status_message.as_deref(),
        ReadOnly::Reading.refusal_message()
    );
}

/// `⌃P` fired from the Explorer must not touch a document the user isn't
/// looking at: no visible change would result (an unfocused document is
/// already `RevealMode::Never`), so a document silently flipped to
/// `ReadOnly::Reading` would surface no explanation on returning to the
/// editor — no caret, no obvious cause. `commands::reading::toggle` gates
/// on `app.focus() == Pane::Editor` before reading or writing any state,
/// silently, matching `app.rs::refocus_title`'s precedent for a
/// non-user-initiated precondition.
#[test]
fn ctrl_p_from_the_explorer_is_a_silent_no_op() {
    let mut app = app_basic("hello");

    send(&mut app, ctrl('b')); // GlobalCommand::FocusExplorer
    assert_eq!(app.focus(), Pane::Explorer);
    let status_before = app.status_message.clone();

    send(&mut app, ctrl('p'));

    assert_eq!(
        app.active_doc().read_only,
        ReadOnly::No,
        "⌃P from the Explorer must not change the active document's ReadOnly state"
    );
    assert_eq!(
        app.status_message, status_before,
        "⌃P from the Explorer must post no status message"
    );
}

/// Keystroke-driven equivalent of `edit_commands.rs`'s
/// `reading_view_blocks_undo_and_redo` (exercise the
/// real resolver, not `commands::edit::undo`/`redo` directly). `⌘Z`/`⌘⇧Z`
/// resolve to `Command::Undo`/`Command::Redo` in `handle_editor_key`'s
/// stage 3, and both guard on `ReadOnly::Reading` before touching the
/// buffer.
#[test]
fn ctrl_z_and_ctrl_shift_z_are_blocked_in_reading_view() {
    let mut app = app_basic("hello");

    send(&mut app, plain(KeyCode::End));
    send(&mut app, plain(KeyCode::Char('!')));
    assert_eq!(app.active_doc().buffer.content(), "hello!");

    app.active_doc_mut().read_only = ReadOnly::Reading;
    let content_before = app.active_doc().buffer.content().to_string();
    let dirty_before = app.active_doc().is_dirty();

    send(&mut app, sup('z'));
    assert_eq!(
        app.active_doc().buffer.content(),
        content_before,
        "⌘Z must not mutate a reading-view document"
    );
    assert_eq!(
        app.active_doc().is_dirty(),
        dirty_before,
        "a blocked undo must not change dirtiness"
    );

    send(&mut app, sup_shift('z'));
    assert_eq!(
        app.active_doc().buffer.content(),
        content_before,
        "⌘⇧Z must not mutate a reading-view document"
    );
    assert_eq!(
        app.active_doc().is_dirty(),
        dirty_before,
        "a blocked redo must not change dirtiness"
    );
}
