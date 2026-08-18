#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use rune_db::SyncKind;
use rune_fuzz::Session;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::merge::MergeState;

use merge_common::{
    bare, ch, chord, ctrl, external_write, messages_posts, next_hunk, reprobe, untitled_draft,
};

const ALT_SUP: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: true,
};

fn open_hello() -> (Session, DocumentId, DocumentId) {
    let session = Session::open("/doc.md", "hello\nfoo\nbar\n");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);
    (session, doc_id, draft_id)
}

fn enter_one_conflict_merge() -> (Session, DocumentId) {
    let (mut session, doc_id, draft_id) = open_hello();
    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());

    external_write(session.app().vfs.as_ref(), b"disk changed this\nfoo\nbar\n");
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());
    assert!(matches!(session.app().merge, MergeState::Active { .. }));
    (session, doc_id)
}

/// `AddCursorAbove` (⌥⌘↑) during an active merge must reach the ordinary
/// editor command, not be silently swallowed by the merge-mode key
/// intercept.
#[test]
fn add_cursor_above_is_not_swallowed_during_an_active_merge() {
    let (mut session, doc_id) = enter_one_conflict_merge();

    // The conflict lands the caret on line 0, where `AddCursorAbove` is a
    // legitimate no-op — move down first so there's a real line above it.
    assert!(session.key(bare(KeyCode::Down)).is_none());
    assert!(session.key(bare(KeyCode::Down)).is_none());

    let cursors_before = session.app().doc(doc_id).unwrap().cursors.len();

    assert!(session.key(chord(KeyCode::Up, ALT_SUP)).is_none());

    let cursors_after = session.app().doc(doc_id).unwrap().cursors.len();
    assert!(
        cursors_after > cursors_before,
        "AddCursorAbove during an active merge must add a cursor, not be swallowed silently: before={cursors_before} after={cursors_after}"
    );
}

/// Navigating to the next conflict when there is only one, and already
/// sitting on it, must still post a status message — never a silent
/// no-op.
#[test]
fn next_conflict_at_the_only_conflict_still_posts_a_message() {
    let (mut session, _doc_id) = enter_one_conflict_merge();
    let posts_before = messages_posts(&session);

    assert!(session.key(next_hunk()).is_none());

    let posts_after = messages_posts(&session);
    assert!(
        posts_after > posts_before,
        "pressing next-conflict at the only conflict must give feedback, not silence: before={posts_before} after={posts_after}"
    );
}
