//! Rename refusal paths — split out of `rename_bind.rs` (plan WP5,
//! 500-line budget), driven through `rune_fuzz::Session`. Every refusal
//! here leaves the machine `Idle`, `file_path` unchanged, the buffer
//! byte-identical, and — after draining every deferred `Cmd` and store op
//! — the published file untouched (`rename_common::assert_refused`).

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
    assert_refused, bound_session, commit_name, ctrl_key, open_title, plain_key, sup_key,
};

/// Decision 12: a read-only document's title cannot be focused AT ALL — the
/// refusal now happens at `^r` itself (`App::focus_title`), before there is
/// ever anything to type. Focusing the Help document's title would
/// otherwise hold the user in a field describing a document they can never
/// rename; removing the illegal state beats guarding it later inside
/// `rename::begin`. `ReadOnly::Always` has no key-reachable setup on a
/// file-backed document, so the variant is set directly.
#[test]
fn a_read_only_document_refuses_to_rename() {
    let (mut session, _mem) = bound_session();
    session.app_mut().active_doc_mut().read_only = ReadOnly::Always;
    let before = session.app().active_doc().buffer.content().to_string();

    assert!(session.key(ctrl_key('r')).is_none());

    assert_eq!(
        session.app().focus(),
        Pane::Editor,
        "the title must never gain focus"
    );
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("this document is read-only")
    );
    assert_eq!(session.app().active_doc().buffer.content(), before);
}

/// A `Preview` document's title cannot be focused either — same mechanism
/// as `Always` above, `App::focus_title`'s generic `ReadOnly` refusal, now
/// reached via a different variant.
#[test]
fn a_preview_document_refuses_to_rename() {
    let (mut session, _mem) = bound_session();
    session.app_mut().active_doc_mut().read_only = ReadOnly::Preview;
    let before = session.app().active_doc().buffer.content().to_string();

    assert!(session.key(ctrl_key('r')).is_none());

    assert_eq!(
        session.app().focus(),
        Pane::Editor,
        "the title must never gain focus"
    );
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        ReadOnly::Preview.refusal_message()
    );
    assert_eq!(session.app().active_doc().buffer.content(), before);
}

/// Decision 12: the Help document is read-only, so its title can never gain
/// focus at all — `^r` refuses with a status instead, and the title row
/// still reads "Help".
#[test]
fn the_help_document_refuses_title_focus() {
    let (mut session, _mem) = bound_session();

    assert!(session.key(plain_key(KeyCode::F1)).is_none());
    assert_eq!(session.app().active_doc().file_name(), "Help");

    assert!(session.key(ctrl_key('r')).is_none());

    assert_eq!(
        session.app().focus(),
        Pane::Editor,
        "a read-only document's title must never gain focus"
    );
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("this document is read-only")
    );
    assert_eq!(
        session.app().active_doc().file_name(),
        "Help",
        "the title row must still read Help"
    );
}

/// A save in flight refuses to rename — reached through the real flow: an
/// edit, ⌘S, then a rename attempt before the save's ack ever lands.
#[test]
fn a_save_in_flight_refuses_to_rename() {
    let (mut session, mem) = bound_session();
    assert!(session.type_("!").is_none());
    assert!(session.key(sup_key('s')).is_none());
    assert!(
        session.app().active_doc().save_in_flight(),
        "test setup: the save must be in flight"
    );
    let before = session.app().active_doc().buffer.content().to_string();

    commit_name(&mut session, "b");

    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("can't rename while a save is in flight")
    );
    assert_refused(&mut session, &mem, &before);
    assert!(
        rune_vfs::Vfs::read(mem.as_ref(), std::path::Path::new("/root/b.md")).is_err(),
        "no file may appear under the refused name"
    );
}

/// A fully empty name is now only reachable with the extension gate
/// unlocked — locked, the extension always leaves at least a dot behind
/// (`title::TitleField::window`'s fenced-off tail), and `.md` alone is a
/// perfectly valid dotfile name, not an empty one.
#[test]
fn an_empty_name_refuses_to_rename() {
    let (mut session, mem) = bound_session();
    let before = session.app().active_doc().buffer.content().to_string();

    open_title(&mut session);
    assert!(session.key(plain_key(KeyCode::Right)).is_none()); // unlock: cursor sits at the split
    assert!(session.key(ctrl_key('a')).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert_eq!(session.app().title.text(), "");
    assert!(session.key(plain_key(KeyCode::Enter)).is_none());

    assert_eq!(
        session.app().focus(),
        Pane::Title,
        "a refused commit must not release focus"
    );
    assert_refused(&mut session, &mem, &before);
}

/// `/` is filtered at the keystroke, so it can never even reach the name —
/// the field's own validation is the second line of defence.
#[test]
fn a_slash_never_enters_the_field() {
    let (mut session, _mem) = bound_session();

    open_title(&mut session);
    assert!(session.type_("b/c").is_none());
    assert_eq!(
        session.app().title.text(),
        "abc.md",
        "'/' must be filtered at the keystroke"
    );
}

/// Committing an unchanged name is a plain refocus, never a rename of a
/// file onto its own path.
#[test]
fn an_unchanged_name_refuses_to_rename() {
    let (mut session, mem) = bound_session();
    let before = session.app().active_doc().buffer.content().to_string();

    open_title(&mut session);
    assert!(session.key(plain_key(KeyCode::Enter)).is_none());

    assert_eq!(session.app().focus(), Pane::Editor);
    assert_refused(&mut session, &mem, &before);
}
