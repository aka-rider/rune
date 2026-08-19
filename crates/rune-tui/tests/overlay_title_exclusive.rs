#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;
use std::sync::Arc;

use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_vfs::{Mem, Vfs};

use rename_common::{
    DOC_CONTENT, bound_session, ctrl_key, draft_session, open_title, plain_key, store_session,
};

fn palette_chord() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('P'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    }
}

fn empty_the_title(session: &mut rune_fuzz::Session) {
    open_title(session);
    assert!(session.key(ctrl_key('a')).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert!(session.key(plain_key(KeyCode::Right)).is_none());
    assert!(session.key(ctrl_key('a')).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert_eq!(
        session.app().title.text(),
        "",
        "the field must hold a name `rename::begin` refuses"
    );
}

#[test]
fn refused_blur_cannot_open_palette_over_title() {
    let (mut session, _mem) = bound_session();
    empty_the_title(&mut session);

    session.key(palette_chord());

    assert!(
        !(session.app().focus() == Pane::Title && session.app().palette().is_some()),
        "the palette must never be open while the title holds focus"
    );
    assert!(
        rune_tui::messages::newest_text(session.app()).is_some(),
        "a refused overlay open must say why"
    );
}

#[test]
fn palette_editor_command_never_fires_with_title_focus() {
    let (mut session, _mem) = bound_session();
    empty_the_title(&mut session);

    session.key(palette_chord());
    session.type_("delete line");
    session.key(plain_key(KeyCode::Enter));

    assert_eq!(
        session.app().active_doc().buffer.content(),
        DOC_CONTENT,
        "a palette command must never reach a document the title is renaming"
    );
}

#[test]
fn rename_ack_does_not_refocus_title_under_overlay() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/seed.md"), b"seed")
        .expect("seed the session's own bootstrap document");
    mem.save_atomic(Path::new("/taken.md"), b"theirs")
        .expect("seed the colliding target");
    let mut session = store_session(&mem, "/root/seed.md");

    rune_tui::workspace::new_untitled_document(session.app_mut());
    session.app_mut().active_doc_mut().focused = true;
    open_title(&mut session);
    assert!(session.type_("taken").is_none());
    assert!(session.key(plain_key(KeyCode::Enter)).is_none());
    assert_eq!(session.app().focus(), Pane::Editor);

    session.key(palette_chord());
    assert!(session.app().palette().is_some(), "the palette must open");

    session.deliver();

    assert_eq!(
        session.app().focus(),
        Pane::Editor,
        "an async rename reply must not plant title focus under an open overlay"
    );
    assert!(
        rune_tui::messages::newest_text(session.app()).is_some(),
        "the collision must still be reported"
    );
}

#[test]
fn a_guard_answered_from_an_open_search_bar_reaches_the_title() {
    let (mut session, _mem) = draft_session();
    assert!(session.type_("draft body").is_none());

    assert!(session.key(ctrl_key('f')).is_none());
    assert!(
        session.app().search_draft().is_some(),
        "the search bar must own focus before the guard is raised"
    );

    session.key(ctrl_key('c'));
    session.key(ctrl_key('c'));
    assert!(
        session.app().guard.is_some(),
        "the dirty-quit guard must arm"
    );
    assert!(
        session.app().search_draft().is_none(),
        "raising a guard must close the search bar"
    );

    session.key(plain_key(KeyCode::Char('s')));

    assert_eq!(
        session.app().focus(),
        Pane::Title,
        "answering [S] on a pathless draft must land focus in the title"
    );
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("name this document to save it \u{2014} press Enter when done")
    );
}
