//! WP6 "Done when" integration tests for undo/redo resync and auto-exit
//! (plan `merge-user-s-changes-with-idempotent-octopus.md`). The
//! disk-conflict Guard tests live in `merge_disk_conflict_guard.rs` — split
//! out to keep both files under the 500-line budget. Follows
//! `merge_entry.rs`/`merge_resolver.rs`'s own fixture pattern, pulling
//! shared setup from `merge_common` (review fix F9's dedupe).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;
use std::sync::Arc;

use rune_db::SyncKind;
use rune_tui::app::App;
use rune_tui::db::DbBridge;
use rune_tui::document::DocumentId;
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use merge_common::{
    app_with_store, bare, ch, ctrl, drain_all_ops_for, drain_one_op_for, external_write, press_key,
    publish,
};

/// Three separate conflicts (lines 1, 5, 9), each surrounded by three
/// unchanged context lines — the same spacing `merge_resolver.rs`'s own
/// two-conflict fixture uses, extended by one more conflict so accepting two
/// of the three leaves the resolver `Active` (the third still unresolved)
/// rather than auto-exiting (decision 13).
const ANCESTOR: &[u8] = b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\nsix\nseven\neight\nnine disk\n";

fn enter_three_conflict_merge() -> (App, Arc<DbBridge>, DocumentId) {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), ANCESTOR);
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("merge-resync", Arc::clone(&vfs));
    let draft_id = app.active;
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);

    press_key(&mut app, ch('X'));
    for _ in 0..4 {
        press_key(&mut app, bare(KeyCode::Down));
    }
    press_key(&mut app, bare(KeyCode::Home));
    press_key(&mut app, ch('X'));
    for _ in 0..4 {
        press_key(&mut app, bare(KeyCode::Down));
    }
    press_key(&mut app, bare(KeyCode::Home));
    press_key(&mut app, ch('X'));
    assert_eq!(
        app.doc(doc_id).unwrap().buffer.content(),
        "Xone\ntwo\nthree\nfour\nXfive\nsix\nseven\neight\nXnine\n"
    );
    drain_all_ops_for(&mut app, &bridge, doc_id);

    external_write(vfs.as_ref(), THEIRS);
    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Diverged));

    app.active = doc_id;
    press_key(&mut app, ctrl('m'));
    drain_one_op_for(&mut app, &bridge, doc_id);

    let MergeState::Active { blocks, cur, .. } = &app.merge else {
        panic!("expected an active resolver, got {:?}", app.merge);
    };
    assert_eq!(blocks.len(), 3, "fixture must produce three conflicts");
    assert_eq!(*cur, 0);
    (app, bridge, doc_id)
}

/// Plan WP6 "Done when" (1): undo after two accepts reopens exactly the
/// last-accepted hunk — `Active` again, `cur` on it, unresolved.
#[test]
fn undo_after_two_accepts_reopens_the_last_accepted_hunk() {
    let (mut app, _bridge, doc_id) = enter_three_conflict_merge();

    press_key(&mut app, ch('o')); // block 0 -> resolved, cur -> 1
    let pos_after_first_accept = app.doc(doc_id).unwrap().journal.pos();
    press_key(&mut app, ch('t')); // block 1 -> resolved, cur -> 2
    let MergeState::Active { blocks, cur, .. } = &app.merge else {
        panic!("resolver must still be active — one conflict (block 2) remains");
    };
    assert!(blocks[0].resolved && blocks[1].resolved && !blocks[2].resolved);
    assert_eq!(*cur, 2);

    rune_tui::commands::edit::undo(&mut app, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().journal.pos(),
        pos_after_first_accept,
        "undo must reverse exactly the second accept's one journal step"
    );

    let MergeState::Active { blocks, cur, .. } = &app.merge else {
        panic!("resync must leave the resolver active");
    };
    assert!(
        !blocks[1].resolved,
        "the just-undone block must be unresolved again"
    );
    assert_eq!(*cur, 1, "cur must land back on the reopened hunk");
    assert!(blocks[0].resolved, "the OTHER accept must be untouched");
}

/// Review fix F1(a): a resolver-Active `undo` used to rescan and reopen
/// EVERY block, not just the one the journal jump touched — a `[B]`'d
/// block is byte-identical in the buffer to an undecided one, so a scan
/// alone cannot tell them apart. `b` (block 0, pushes no journal step)
/// then `o` (block 1, pushes one) then `undo` must reopen ONLY block 1
/// (the O-accept undo actually reversed) and leave block 0's `B`
/// verdict untouched.
#[test]
fn undo_after_both_then_ours_reopens_only_the_ours_block_not_the_both_block() {
    let (mut app, _bridge, doc_id) = enter_three_conflict_merge();

    press_key(&mut app, ch('b')); // block 0 -> resolved, no journal step
    let pos_before_ours = app.doc(doc_id).unwrap().journal.pos();
    press_key(&mut app, ch('o')); // block 1 -> resolved, one journal step
    let MergeState::Active { blocks, .. } = &app.merge else {
        panic!("resolver must still be active — block 2 remains");
    };
    assert!(blocks[0].resolved && blocks[1].resolved && !blocks[2].resolved);

    rune_tui::commands::edit::undo(&mut app, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().journal.pos(),
        pos_before_ours,
        "undo must reverse exactly the ours-accept's one journal step"
    );

    let MergeState::Active { blocks, .. } = &app.merge else {
        panic!("resync must leave the resolver active — block 2 is still unresolved");
    };
    assert!(
        !blocks[1].resolved,
        "the just-undone ours-accepted block must be unresolved again"
    );
    assert!(
        blocks[0].resolved,
        "the untouched B-resolved block must stay resolved — this is review finding F1"
    );
}

/// Review fix F1(c): `[B]` pushes no journal step, so `undo` right after a
/// `B` actually reverses whatever the PREVIOUS real edit was — here, an
/// earlier `o` accept on a different block. That undo's affected range
/// still must not touch the later `B`'d block's verdict.
#[test]
fn undo_after_ours_then_both_reopens_only_the_ours_block_not_the_both_block() {
    let (mut app, _bridge, doc_id) = enter_three_conflict_merge();

    let pos_before_ours = app.doc(doc_id).unwrap().journal.pos();
    press_key(&mut app, ch('o')); // block 0 -> resolved, one journal step
    press_key(&mut app, ch('b')); // block 1 -> resolved, no journal step
    let MergeState::Active { blocks, .. } = &app.merge else {
        panic!("resolver must still be active — block 2 remains");
    };
    assert!(blocks[0].resolved && blocks[1].resolved && !blocks[2].resolved);

    // `B` pushed no step, so this undo reverses the EARLIER `o` accept.
    rune_tui::commands::edit::undo(&mut app, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().journal.pos(),
        pos_before_ours,
        "undo must reverse the earlier ours-accept's journal step, since B pushed none"
    );

    let MergeState::Active { blocks, .. } = &app.merge else {
        panic!("resync must leave the resolver active — block 2 is still unresolved");
    };
    assert!(
        !blocks[0].resolved,
        "the just-undone ours-accepted block must be unresolved again"
    );
    assert!(
        blocks[1].resolved,
        "the B-resolved block must stay resolved even though it postdates the undone step"
    );
}

/// Plan WP6 "Done when" (2): a document whose PROSE quotes literal
/// `<<<<<<< editor`/`=======`/`>>>>>>> disk` lines round-trips resync
/// without misclassifying (port of the Go reference resync test's quoted-marker
/// scenario) — driven end-to-end through a real undo/redo, not just the
/// unit-level `resync` module tests.
#[test]
fn undo_redo_round_trips_when_prose_quotes_literal_marker_lines() {
    let quoted_ours =
        "a quoted example:\n<<<<<<< editor\nfake ours\n=======\nfake theirs\n>>>>>>> disk\nend";
    let ancestor = b"intro\nSTART\noutro\n".to_vec();
    let theirs = b"intro\ntheirs replacement\noutro\n".to_vec();

    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), &ancestor);
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("merge-resync-quoted", Arc::clone(&vfs));
    let draft_id = app.active;
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);

    // Replace the whole buffer with the quoted-marker `ours` text: select
    // all, then type over the selection one rune/`Enter` at a time.
    rune_tui::commands::nav::select_all(app.doc_mut(doc_id).unwrap());
    for c in quoted_ours.chars() {
        if c == '\n' {
            press_key(&mut app, bare(KeyCode::Enter));
        } else {
            press_key(&mut app, ch(c));
        }
    }
    assert_eq!(app.doc(doc_id).unwrap().buffer.content(), quoted_ours);
    drain_all_ops_for(&mut app, &bridge, doc_id);

    external_write(vfs.as_ref(), &theirs);
    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    drain_one_op_for(&mut app, &bridge, doc_id);

    app.active = doc_id;
    press_key(&mut app, ctrl('m'));
    drain_one_op_for(&mut app, &bridge, doc_id);

    let MergeState::Active { blocks, .. } = &app.merge else {
        panic!("expected an active resolver, got {:?}", app.merge);
    };
    assert_eq!(
        blocks.len(),
        1,
        "the quoted marker prose is INSIDE ours — one real conflict"
    );

    let pre_accept_bytes = app.doc(doc_id).unwrap().buffer.content().to_string();
    press_key(&mut app, ch('t')); // accept theirs — resolves the only block, auto-exits
    assert_eq!(app.merge, MergeState::Inactive);
    let post_accept_bytes = app.doc(doc_id).unwrap().buffer.content().to_string();
    assert!(post_accept_bytes.contains("theirs replacement"));

    rune_tui::commands::edit::undo(&mut app, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().buffer.content(),
        pre_accept_bytes,
        "undo must restore the quoted-marker working form byte-for-byte"
    );

    rune_tui::commands::edit::redo(&mut app, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().buffer.content(),
        post_accept_bytes,
        "redo must reapply the accept byte-for-byte"
    );
}

/// Plan WP6 "Done when" (3): switching tabs mid-merge exits merge in place,
/// keeps the buffer bytes exactly, and reverts the tab title.
#[test]
fn tab_switch_mid_merge_exits_merge_keeps_bytes_and_reverts_title() {
    let (mut app, _bridge, doc_id) = enter_three_conflict_merge();
    let other = app.open_document(rune_core::buffer::Buffer::new("scratch"));

    let bytes_before = app.doc(doc_id).unwrap().buffer.content().to_string();
    assert!(
        app.doc(doc_id)
            .unwrap()
            .file_name()
            .ends_with(": editor <-> disk")
    );

    workspace::switch_to(&mut app, other);

    assert_eq!(app.merge, MergeState::Inactive);
    assert_eq!(
        app.doc(doc_id).unwrap().buffer.content(),
        bytes_before,
        "auto-exit must never touch the buffer"
    );
    assert!(
        !app.doc(doc_id)
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
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("merge-resync-pending-cancel", Arc::clone(&vfs));
    let draft_id = app.active;
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);

    press_key(&mut app, ch('!'));
    drain_one_op_for(&mut app, &bridge, doc_id);
    external_write(vfs.as_ref(), b"disk changed this");
    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Diverged));

    app.active = doc_id;
    press_key(&mut app, ctrl('m'));
    assert!(
        matches!(app.merge, MergeState::Pending { doc, .. } if doc == doc_id),
        "expected a Pending merge attempt, got {:?}",
        app.merge
    );

    // Switch away BEFORE the MergePrep ack ever arrives.
    workspace::switch_to(&mut app, draft_id);

    assert_eq!(
        app.merge,
        MergeState::Inactive,
        "a Pending attempt must be cancelled, not left dangling"
    );
    assert!(
        app.status_message
            .as_deref()
            .unwrap_or_default()
            .contains("cancelled"),
        "expected a cancellation status, got {:?}",
        app.status_message
    );

    // The now-stale MergePrep ack must land safely: no panic, and it must
    // NOT resurrect `Active` on a document the user has switched away from.
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert_eq!(app.merge, MergeState::Inactive);
    assert_eq!(app.active, draft_id);
}
