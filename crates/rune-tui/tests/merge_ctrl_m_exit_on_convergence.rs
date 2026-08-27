//! Regression: `^M` (`GlobalCommand::Merge`) must still be able to CLOSE an
//! `Active` merge session once the disk has quietly re-converged.
//! `avail::merge` used to gate solely on `is_divergent`, so the moment a
//! probe reported `Clean` again — with at least one conflict already
//! resolved, which keeps `retract_active_on_convergence` from retiring the
//! session on its own — the palette row greyed out and `pane_command`'s
//! `registry_refusal` check refused the chord outright: the advertised exit
//! key went dead, even though Escape still worked. Driven through
//! `rune_fuzz::Session`'s real key pipeline, the same route the palette row
//! and the chord both go through (`registry::availability`).
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
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;

use merge_common::{
    bare, ch, ctrl, external_write, messages_posts, reprobe, take_ours, untitled_draft,
};

/// Probes `doc` in place, without `merge_common::reprobe`'s away-and-back
/// switch — switching away from the document a merge is `Active` on runs
/// `workspace::switch_to`'s own auto-exit, which would tear the very
/// session down this test means to reprobe underneath.
fn reprobe_in_place(session: &mut Session, doc: DocumentId) {
    rune_tui::db_enqueue::probe(session.app_mut(), doc);
    assert!(session.deliver_db().is_none());
}

const ANCESTOR: &str = "one\ntwo\nthree\nfour\nfive\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\n";

fn enter_two_conflict_merge() -> (Session, DocumentId) {
    let mut session = Session::open("/doc.md", ANCESTOR);
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('X')).is_none());
    for _ in 0..4 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::End)).is_none());
    assert!(session.key(ch('Z')).is_none());
    assert!(session.deliver_db_all().is_none());

    external_write(session.app().vfs.as_ref(), THEIRS);
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db_all().is_none());
    assert!(
        matches!(&session.app().merge, MergeState::Active { session, .. } if session.conflicts.len() == 2),
        "fixture must enter a two-conflict merge, got {:?}",
        session.app().merge
    );
    (session, doc_id)
}

#[test]
fn ctrl_m_closes_an_active_merge_once_the_disk_reconverges_with_a_hunk_still_unresolved() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    // Resolve ONE of the two conflicts — `retract_active_on_convergence`
    // will now decline to retire the session on its own once the disk
    // reconverges, since something has already been resolved.
    assert!(session.key(take_ours()).is_none());
    assert!(session.deliver_db_all().is_none());
    assert!(matches!(&session.app().merge, MergeState::Active { .. }));

    // An external process rewrites the disk to match the buffer's CURRENT
    // bytes exactly — `SyncKind::Clean` compares against the buffer's own
    // content, not the original ancestor, so this (not reverting to
    // `ANCESTOR`, which would still read as `Diverged` against the edited
    // buffer) is what a real reconvergence looks like here.
    let converged = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    external_write(session.app().vfs.as_ref(), converged.as_bytes());
    reprobe_in_place(&mut session, doc_id);

    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "test setup: the disk is no longer divergent"
    );
    assert!(
        matches!(&session.app().merge, MergeState::Active { .. }),
        "test setup: something already resolved keeps the session up despite the reconverge"
    );

    let posts_before = messages_posts(&session);
    assert!(session.key(ctrl('m')).is_none());

    assert_eq!(
        session.app().merge,
        MergeState::Inactive,
        "^M must close the resting Active session once the disk has reconverged"
    );
    assert!(
        messages_posts(&session) > posts_before,
        "the exit must leave observable feedback, not a silent swallow"
    );
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("unresolved"),
        "expected the abandon-with-unresolved-conflicts status, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
}

/// Entry is unaffected by the fix: with no session up and nothing
/// divergent, `^M` still refuses exactly as before.
#[test]
fn ctrl_m_still_refuses_entry_without_divergence() {
    let mut session = Session::open("/doc.md", "hello");

    assert!(session.key(ctrl('m')).is_none());

    assert_eq!(session.app().merge, MergeState::Inactive);
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("no divergence to merge")
    );
}
