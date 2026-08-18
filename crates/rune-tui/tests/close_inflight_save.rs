//! Close-vs-inflight-save ownership: `^w` on a CLEAN document whose save is
//! still in flight must not sweep the save's op bookkeeping out from under
//! the eventual ack — the close waits for the ack and completes then, so
//! the write is recorded and no `db_ops` entry is orphaned. Driven through
//! `rune_fuzz::Session` against a real in-memory `Store`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use rune_fuzz::Session;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

const SAVE: KeyInput = KeyInput {
    code: KeyCode::Char('s'),
    mods: SUP,
};

const UNDO: KeyInput = KeyInput {
    code: KeyCode::Char('z'),
    mods: SUP,
};

const CLOSE: KeyInput = KeyInput {
    code: KeyCode::Char('w'),
    mods: CTRL,
};

#[test]
fn closing_a_clean_doc_with_a_save_in_flight_settles_the_ack_then_closes() {
    let mut session = Session::open("/doc.md", "hello");
    let id = session.app().active;

    assert!(session.type_("X").is_none());
    assert!(session.deliver_db_all().is_none());

    assert!(session.key(SAVE).is_none());
    assert!(
        session.app().doc(id).unwrap().save_in_flight(),
        "the save is armed but its prepare ack is deliberately undelivered"
    );

    assert!(session.key(UNDO).is_none());
    assert!(session.key(CLOSE).is_none());

    assert!(
        session.app().doc(id).is_some(),
        "a clean doc with a save in flight must not close under the ack"
    );
    assert_eq!(session.app().pending_close_on_save, Some(id));
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("save in progress \u{2014} closing once it completes")
    );

    assert!(session.deliver_db_all().is_none());
    assert!(session.deliver().is_none());
    assert!(session.deliver_db_all().is_none());

    assert!(
        session.app().doc(id).is_none(),
        "the resolved save completes the deferred close"
    );
    assert!(session.app().pending_close_on_save.is_none());
    assert!(
        session.app().db_ops.is_empty(),
        "no orphaned db_ops entry survives the settled close"
    );
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"Xhello",
        "the in-flight save the user confirmed with \u{2318}S still lands"
    );
}
