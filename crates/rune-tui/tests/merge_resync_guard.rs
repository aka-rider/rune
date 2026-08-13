//! WP6 "Done when" integration tests for undo/redo resync and auto-exit
//! (plan `merge-user-s-changes-with-idempotent-octopus.md`). The
//! disk-conflict Guard tests live in `merge_disk_conflict_guard.rs` — split
//! out to keep both files under the 500-line budget. Driven through
//! `rune_fuzz::Session`, pulling shared setup from `merge_common`.
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
use rune_tui::workspace;

use merge_common::{bare, ch, ctrl, external_write, reprobe, sup, untitled_draft};

/// Three separate conflicts (lines 1, 5, 9), each surrounded by three
/// unchanged context lines — the same spacing `merge_resolver.rs`'s own
/// two-conflict fixture uses, extended by one more conflict so accepting two
/// of the three leaves the resolver `Active` (the third still unresolved)
/// rather than auto-exiting (decision 13).
const ANCESTOR: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\nsix\nseven\neight\nnine disk\n";

fn enter_three_conflict_merge() -> (Session, DocumentId) {
    let mut session = Session::open("/doc.md", ANCESTOR);
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('X')).is_none());
    for _ in 0..4 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::Home)).is_none());
    assert!(session.key(ch('X')).is_none());
    for _ in 0..4 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::Home)).is_none());
    assert!(session.key(ch('X')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "Xone\ntwo\nthree\nfour\nXfive\nsix\nseven\neight\nXnine\n"
    );
    assert!(session.deliver_db_all().is_none());

    external_write(session.app().vfs.as_ref(), THEIRS);
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());

    let MergeState::Active { pairs, cur, .. } = &session.app().merge else {
        panic!("expected an active resolver, got {:?}", session.app().merge);
    };
    assert_eq!(pairs.len(), 3, "fixture must produce three conflicts");
    assert_eq!(*cur, 0);
    (session, doc_id)
}

/// Plan WP6 "Done when" (1): undo after two accepts reopens exactly the
/// last-accepted hunk — `Active` again, `cur` on it, unresolved. Undo
/// mid-merge is not reachable through the key pipeline (the resolver's
/// intercept swallows `⌘Z`), so the command entry point is the seam under
/// test here and below.
#[test]
fn undo_after_two_accepts_reopens_the_last_accepted_hunk() {
    let (mut session, doc_id) = enter_three_conflict_merge();

    assert!(session.key(ch('o')).is_none()); // block 0 -> resolved, cur -> 1
    let pos_after_first_accept = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(ch('t')).is_none()); // block 1 -> resolved, cur -> 2
    let MergeState::Active { pairs, cur, .. } = &session.app().merge else {
        panic!("resolver must still be active — one conflict (block 2) remains");
    };
    assert!(pairs[0].block.resolved && pairs[1].block.resolved && !pairs[2].block.resolved);
    assert_eq!(*cur, 2);

    rune_tui::commands::edit::undo(session.app_mut(), doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().journal.pos(),
        pos_after_first_accept,
        "undo must reverse exactly the second accept's one journal step"
    );

    let MergeState::Active { pairs, cur, .. } = &session.app().merge else {
        panic!("resync must leave the resolver active");
    };
    assert!(
        !pairs[1].block.resolved,
        "the just-undone block must be unresolved again"
    );
    assert_eq!(*cur, 1, "cur must land back on the reopened hunk");
    assert!(
        pairs[0].block.resolved,
        "the OTHER accept must be untouched"
    );
}

/// Review fix F1(a): a resolver-Active `undo` used to rescan and reopen
/// EVERY block, not just the one the journal jump touched. `b` (block 0)
/// then `o` (block 1) then `undo` must reopen ONLY block 1
/// (the O-accept undo actually reversed) and leave block 0's `B`
/// verdict untouched.
#[test]
fn undo_after_both_then_ours_reopens_only_the_ours_block_not_the_both_block() {
    let (mut session, doc_id) = enter_three_conflict_merge();

    assert!(session.key(ch('b')).is_none()); // block 0 -> resolved, one journal step
    let pos_before_ours = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(ch('o')).is_none()); // block 1 -> resolved, one journal step
    let MergeState::Active { pairs, .. } = &session.app().merge else {
        panic!("resolver must still be active — block 2 remains");
    };
    assert!(pairs[0].block.resolved && pairs[1].block.resolved && !pairs[2].block.resolved);

    rune_tui::commands::edit::undo(session.app_mut(), doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().journal.pos(),
        pos_before_ours,
        "undo must reverse exactly the ours-accept's one journal step"
    );

    let MergeState::Active { pairs, .. } = &session.app().merge else {
        panic!("resync must leave the resolver active — block 2 is still unresolved");
    };
    assert!(
        !pairs[1].block.resolved,
        "the just-undone ours-accepted block must be unresolved again"
    );
    assert!(
        pairs[0].block.resolved,
        "the untouched B-resolved block must stay resolved — this is review finding F1"
    );
}

/// `[B]` is an ordinary journaled edit now, so `undo` right after a `B`
/// reverses exactly that accept — reopening ONLY the `B`'d block (its
/// framed markers return) and leaving the earlier `o` accept untouched.
#[test]
fn undo_after_ours_then_both_reopens_only_the_both_block() {
    let (mut session, doc_id) = enter_three_conflict_merge();

    assert!(session.key(ch('o')).is_none()); // block 0 -> resolved, one journal step
    let pos_before_both = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(ch('b')).is_none()); // block 1 -> resolved, one journal step
    let MergeState::Active { pairs, .. } = &session.app().merge else {
        panic!("resolver must still be active — block 2 remains");
    };
    assert!(pairs[0].block.resolved && pairs[1].block.resolved && !pairs[2].block.resolved);

    rune_tui::commands::edit::undo(session.app_mut(), doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().journal.pos(),
        pos_before_both,
        "undo must reverse exactly the both-accept's one journal step"
    );

    let MergeState::Active { pairs, .. } = &session.app().merge else {
        panic!("resync must leave the resolver active — block 2 is still unresolved");
    };
    assert!(
        !pairs[1].block.resolved,
        "the just-undone both-accepted block must be unresolved again"
    );
    assert!(
        session
            .app()
            .doc(doc_id)
            .unwrap()
            .buffer
            .content()
            .matches("<<<<<<<")
            .count()
            >= 2,
        "the undone block's framed markers must return"
    );
    assert!(
        pairs[0].block.resolved,
        "the earlier ours accept must be untouched"
    );
}

/// Plan WP6 "Done when" (2): a document whose PROSE quotes literal
/// `<<<<<<< editor`/`=======`/`>>>>>>> disk` lines round-trips resync
/// without misclassifying — driven end-to-end through a real undo/redo,
/// not just the unit-level `resync` module tests.
#[test]
fn undo_redo_round_trips_when_prose_quotes_literal_marker_lines() {
    let quoted_ours =
        "a quoted example:\n<<<<<<< editor\nfake ours\n=======\nfake theirs\n>>>>>>> disk\nend";
    let ancestor = "intro\nSTART\noutro\n";
    let theirs = b"intro\ntheirs replacement\noutro\n";

    let mut session = Session::open("/doc.md", ancestor);
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    // Replace the whole buffer with the quoted-marker `ours` text: select
    // all, then type over the selection one rune/`Enter` at a time.
    assert!(session.key(sup('a')).is_none());
    for c in quoted_ours.chars() {
        if c == '\n' {
            assert!(session.key(bare(KeyCode::Enter)).is_none());
        } else {
            assert!(session.key(ch(c)).is_none());
        }
    }
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        quoted_ours
    );
    assert!(session.deliver_db_all().is_none());

    external_write(session.app().vfs.as_ref(), theirs);
    reprobe(&mut session, draft_id, doc_id);

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());

    let MergeState::Active { pairs, .. } = &session.app().merge else {
        panic!("expected an active resolver, got {:?}", session.app().merge);
    };
    assert_eq!(
        pairs.len(),
        1,
        "the quoted marker prose is INSIDE ours — one real conflict"
    );

    let pre_accept_bytes = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    // Accept theirs — resolves the only block, auto-exits.
    assert!(session.key(ch('t')).is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    let post_accept_bytes = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    assert!(post_accept_bytes.contains("theirs replacement"));

    assert!(session.key(sup('z')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        pre_accept_bytes,
        "undo must restore the quoted-marker working form byte-for-byte"
    );

    assert!(session.key(ctrl('y')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        post_accept_bytes,
        "redo must reapply the accept byte-for-byte"
    );
}

/// Plan WP6 "Done when" (3): switching tabs mid-merge exits merge in place,
/// keeps the buffer bytes exactly, and reverts the tab title.
#[test]
fn tab_switch_mid_merge_exits_merge_keeps_bytes_and_reverts_title() {
    let (mut session, doc_id) = enter_three_conflict_merge();
    let other = session
        .app_mut()
        .open_document(rune_core::buffer::Buffer::new("scratch"));

    let bytes_before = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    assert!(
        session
            .app()
            .doc(doc_id)
            .unwrap()
            .file_name()
            .ends_with(": editor <-> disk")
    );

    workspace::switch_to(session.app_mut(), other);

    assert_eq!(session.app().merge, MergeState::Inactive);
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        bytes_before,
        "auto-exit must never touch the buffer"
    );
    assert!(
        !session
            .app()
            .doc(doc_id)
            .unwrap()
            .file_name()
            .ends_with(": editor <-> disk"),
        "the title must revert on auto-exit"
    );
}

/// Review fix F3: `^M` on a diverged document leaves `app.merge` `Pending`
/// while its `MergePrep` op is still in flight — switching tabs BEFORE
/// that ack lands used to silently discard the attempt with `exit_in_place`
/// (which only knows how to unwind an `Active` working form, not a
/// `Pending` one waiting on disk state) and no feedback at all. It must
/// instead cancel WITH a status, and the eventual (now-stale) ack must be
/// safely dropped rather than resurrecting `Active` on a document the user
/// has since switched away from.
#[test]
fn tab_switch_while_merge_prep_is_still_pending_cancels_with_a_status_and_drops_the_stale_ack() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());
    external_write(session.app().vfs.as_ref(), b"disk changed this");
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(
        matches!(session.app().merge, MergeState::Pending { doc, .. } if doc == doc_id),
        "expected a Pending merge attempt, got {:?}",
        session.app().merge
    );

    // Switch away BEFORE the MergePrep ack ever arrives.
    workspace::switch_to(session.app_mut(), draft_id);

    assert_eq!(
        session.app().merge,
        MergeState::Inactive,
        "a Pending attempt must be cancelled, not left dangling"
    );
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("cancelled"),
        "expected a cancellation status, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );

    // The now-stale MergePrep ack must land safely: no panic, and it must
    // NOT resurrect `Active` on a document the user has switched away from.
    assert!(session.deliver_db().is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert_eq!(session.app().active, draft_id);
}
