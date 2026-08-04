//! Rename refusal paths — split out of `rename_bind.rs` (plan WP5, §1.6)
//! once the extension-gate and clipboard packages grew that file past the
//! ceiling. Every refusal here leaves the machine `Idle`, `file_path`
//! unchanged, and the buffer byte-identical
//! (`rename_common::assert_refused`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use rune_tui::document::ReadOnly;
use rune_tui::keymap::KeyCode;
use rune_tui::pane::Pane;

use rename_common::{
    app_with, assert_refused, ctrl, plain, rename_to, seeded_vfs, send, type_text,
};

/// Decision 12: a read-only document's title cannot be focused AT ALL — the
/// refusal now happens at `^r` itself (`App::focus_title`), before there is
/// ever anything to type. Focusing the Help document's title would
/// otherwise hold the user in a field describing a document they can never
/// rename; removing the illegal state beats guarding it later inside
/// `rename::begin`.
#[test]
fn a_read_only_document_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    app.active_doc_mut().read_only = ReadOnly::Always;
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));

    assert_eq!(app.focus(), Pane::Editor, "the title must never gain focus");
    assert_eq!(
        app.status_message.as_deref(),
        Some("this document is read-only")
    );
    assert_eq!(app.active_doc().buffer.content(), before);
}

/// Plan WP6: a `Preview` document's title cannot be focused either — same
/// mechanism as `Always` above, `App::focus_title`'s generic `ReadOnly`
/// refusal, now reached via a different variant.
#[test]
fn a_preview_document_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    app.active_doc_mut().read_only = ReadOnly::Preview;
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));

    assert_eq!(app.focus(), Pane::Editor, "the title must never gain focus");
    assert_eq!(
        app.status_message.as_deref(),
        ReadOnly::Preview.refusal_message()
    );
    assert_eq!(app.active_doc().buffer.content(), before);
}

/// Decision 12: the Help document is read-only, so its title can never gain
/// focus at all — `^r` refuses with a status instead, and the title row
/// still reads "Help".
#[test]
fn the_help_document_refuses_title_focus() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, plain(KeyCode::F1));
    assert_eq!(app.active_doc().file_name(), "Help");

    send(&mut app, ctrl('r'));

    assert_eq!(
        app.focus(),
        Pane::Editor,
        "a read-only document's title must never gain focus"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("this document is read-only")
    );
    assert_eq!(
        app.active_doc().file_name(),
        "Help",
        "the title row must still read Help"
    );
}

/// The no-store `save_cmd` captures `path` in its closure and would
/// republish at the OLD name, so a rename mid-save is refused.
#[test]
fn a_save_in_flight_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    app.active_doc_mut().save_in_flight = true;
    let before = app.active_doc().buffer.content().to_string();

    let effects = rename_to(&mut app, "b");
    assert_refused(&app, &effects, &before);
}

/// A fully empty name is now only reachable with the extension gate
/// unlocked — locked, the extension always leaves at least a dot behind
/// (`title::TitleField::window`'s fenced-off tail), and `.md` alone is a
/// perfectly valid dotfile name, not an empty one.
#[test]
fn an_empty_name_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));
    send(&mut app, plain(KeyCode::Right)); // unlock: cursor sits at the split
    send(&mut app, ctrl('a'));
    send(&mut app, plain(KeyCode::Backspace));
    assert_eq!(app.title.text(), "");
    let effects = send(&mut app, plain(KeyCode::Enter));
    assert_refused(&app, &effects, &before);
}

/// `/` is filtered at the keystroke, so it can never even reach the name —
/// the field's own validation is the second line of defence.
#[test]
fn a_slash_never_enters_the_field() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    type_text(&mut app, "b/c");
    assert_eq!(
        app.title.text(),
        "abc.md",
        "'/' must be filtered at the keystroke"
    );
}

/// Committing an unchanged name is a plain refocus, never a rename of a
/// file onto its own path.
#[test]
fn an_unchanged_name_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));
    let effects = send(&mut app, plain(KeyCode::Enter));

    assert_eq!(app.focus(), Pane::Editor);
    assert_refused(&app, &effects, &before);
}
