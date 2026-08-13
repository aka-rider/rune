//! A doc-scoped read op failing (a probe against an externally deleted
//! file) surfaces a per-document error message and leaves the recovery
//! store trusted; only real journal failures degrade it. Driven through
//! `rune_fuzz::Session`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;

use rune_fuzz::Session;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::workspace;

const END: KeyInput = KeyInput {
    code: KeyCode::End,
    mods: Mods::NONE,
};

#[test]
fn probe_missing_file_keeps_store() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = session
        .app()
        .documents
        .iter()
        .map(|(&id, _)| id)
        .find(|&id| id != doc_id)
        .expect("the untitled draft stays open alongside the seed");
    assert!(
        session.app().db_ops.is_empty(),
        "test setup: session setup fully drained"
    );

    session
        .app()
        .vfs
        .remove(Path::new("/doc.md"))
        .expect("delete the file out from under the open document");

    workspace::switch_to(session.app_mut(), draft_id);
    workspace::switch_to(session.app_mut(), doc_id);
    let posts_before = rune_tui::messages::posts(session.app());
    assert!(session.deliver_db().is_none());

    assert!(
        rune_tui::messages::posts(session.app()) > posts_before,
        "the probe failure must post a message"
    );
    assert!(
        rune_tui::messages::newest_text(session.app()).is_some_and(|s| s.contains("doc.md")),
        "the error must name the document, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
    assert!(
        session.app().db.as_ref().is_some_and(|d| !d.degraded),
        "a doc-scoped probe failure must never degrade the whole store"
    );
    assert!(
        session.app().db_banner.is_none(),
        "no sticky degraded banner for a missing file, got {:?}",
        session.app().db_banner
    );

    assert!(session.key(END).is_none());
    assert!(session.type_("!").is_none());
    assert!(
        session
            .app()
            .doc(doc_id)
            .unwrap()
            .buffer
            .content()
            .contains('!'),
        "editing must keep working after the failed probe, got {:?}",
        session.app().doc(doc_id).unwrap().buffer.content()
    );
    assert!(session.deliver_db().is_none());
    assert!(
        session.app().db.as_ref().is_some_and(|d| !d.degraded),
        "the store must stay trusted for recovery after the whole sequence"
    );
}
