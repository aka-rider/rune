//! Regression: `merge::begin` used to overwrite an already-`Active` merge
//! session in place — no `diff_view::teardown`, no `display_name` restore,
//! no `merge_close` for the old store row — whenever the `DiskConflict`
//! Guard's `[M]erge` answer landed on a document sitting in the resting
//! `Active`-with-nothing-unresolved state (every conflict hand-resolved,
//! `refuses_save` therefore letting `^S` through, and the disk moving again
//! raising the Guard). `merge::toggle` already tore its own session down
//! cleanly; the Guard's direct `merge::begin` call bypassed that. Driven
//! through `rune_fuzz::Session`'s real key pipeline over a file-backed
//! `Store`, so the fix can be checked against the actual `merges` row, not
//! just in-memory state.
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
use rune_fuzz::Session;
use rune_tui::db::Db;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;
use rune_vfs::{Mem, Vfs};

use merge_common::db_wiring_common::{publish, store_at, temp_db_dir};
use merge_common::{bare, ch, ctrl, external_write, save_and_ack, untitled_draft};

const ANCESTOR: &[u8] = b"one\ntwo\nthree\n";
const THEIRS_ONCE: &[u8] = b"one disk once\ntwo\nthree\n";
const THEIRS_TWICE: &[u8] = b"one disk twice\ntwo\nthree\n";

/// Counts `merges` rows still `active` for `doc_db_id` — a fresh, separate
/// connection onto the SAME (WAL-mode) sqlite file the session's own
/// `Store` is writing through, opened only after every enqueued op has been
/// drained, so there is nothing left in flight to race.
fn active_row_count(db_path: &Path, doc_db_id: i64) -> i64 {
    let conn =
        rune_db::open_raw_connection_at_path_for_test(db_path).expect("open raw db connection");
    conn.query_row(
        "SELECT count(*) FROM merges WHERE doc_id = ?1 AND state = 'active'",
        [doc_db_id],
        |row| row.get(0),
    )
    .expect("count active merge rows")
}

#[test]
fn begin_over_an_active_resolved_merge_tears_down_the_old_session_first() {
    let db_path = temp_db_dir("begin-over-active").join("rune-db.sqlite");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), ANCESTOR);
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let (store, bridge) = store_at(&db_path, Arc::clone(&vfs));
    let mut session =
        Session::open_with_db("/doc.md", Arc::clone(&mem), Db::new(store, bridge, false));
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);
    let original_display_name = session.app().doc(doc_id).unwrap().display_name.clone();

    assert!(session.key(ch('X')).is_none());
    assert!(session.deliver_db_all().is_none());

    external_write(session.app().vfs.as_ref(), THEIRS_ONCE);
    merge_common::reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db_all().is_none());
    match &session.app().merge {
        MergeState::Active { session: merge, .. } => {
            assert_eq!(
                merge.conflicts.len(),
                1,
                "fixture must produce exactly one conflict"
            );
        }
        other => panic!("expected the first merge to land Active, got {other:?}"),
    }
    let doc_db_id = session.app().doc(doc_id).unwrap().doc_db().unwrap().db_id;
    assert_eq!(
        active_row_count(&db_path, doc_db_id),
        1,
        "test setup: the first merge's row is active"
    );

    // Hand-edit the sole conflict to resolve it WITHOUT exiting the merge —
    // reaches the resting `Active`-with-nothing-unresolved state, the same
    // mechanics `merge_resolver.rs`'s own hand-edit tests exercise.
    assert!(session.key(bare(KeyCode::Right)).is_none());
    assert!(session.key(ch('Q')).is_none());
    assert!(session.deliver_db_all().is_none());
    match &session.app().merge {
        MergeState::Active { session: merge, .. } => {
            assert_eq!(
                merge.unresolved_count(),
                0,
                "test setup: the sole conflict is hand-resolved"
            );
        }
        other => panic!("expected the merge to stay Active, got {other:?}"),
    }
    assert_eq!(
        session.app().doc(doc_id).unwrap().display_name.as_deref(),
        Some("doc.md: editor <-> disk"),
        "test setup: the resolver's retitle is up"
    );
    assert!(
        session.app().diff.is_some(),
        "test setup: the resolver pane stays up while resting"
    );

    // The disk moves again while the resolved-but-not-exited merge sits
    // idle — `^S` really attempts a save (`refuses_save` returns false with
    // nothing unresolved) and its CAS raises the disk-conflict Guard.
    external_write(session.app().vfs.as_ref(), THEIRS_TWICE);
    save_and_ack(&mut session);
    let Some(prompt) = &session.app().guard else {
        panic!("expected ^S to raise the disk-conflict Guard");
    };
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));

    // `m` answers the Guard exactly the way `handle_disk_conflict_key` does
    // — the route that used to overwrite `app.merge` in place.
    assert!(session.key(ch('m')).is_none());
    assert!(session.app().guard.is_none());

    // The stale session must already be torn down before the fresh
    // `Pending` even lands: no leftover diff pane, the retitle undone, and
    // its store row closed.
    assert!(
        session.app().diff.is_none(),
        "the first resolver's pane must not survive into the fresh attempt"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().display_name,
        original_display_name,
        "the merge retitle must be undone before a new merge begins"
    );
    assert!(
        matches!(session.app().merge, MergeState::Pending { doc, .. } if doc == doc_id),
        "expected a fresh Pending ticket, got {:?}",
        session.app().merge
    );
    // Teardown enqueued two ops — `resolve_abandon` then `merge_close` (only
    // the latter deactivates the row). Drain exactly those two, stopping
    // short of the fresh attempt's `merge_open`, which would otherwise land
    // in the SAME sweep and mask a still-active old row behind a new one.
    assert!(session.deliver_db().is_none());
    assert!(session.deliver_db().is_none());
    assert_eq!(
        active_row_count(&db_path, doc_db_id),
        0,
        "the torn-down first session's row must not be left active"
    );

    // The fresh merge lands normally against the LATEST disk content.
    assert!(session.deliver_db_all().is_none());
    match &session.app().merge {
        MergeState::Active { session: merge, .. } => {
            assert_eq!(merge.conflicts.len(), 1);
        }
        other => panic!("expected the fresh merge to land Active, got {other:?}"),
    }
    let diff = session
        .app()
        .diff
        .as_ref()
        .expect("the fresh resolver installs its own pane");
    assert!(
        diff.left.buffer.content().contains("disk twice"),
        "the fresh resolver must show the LATEST disk bytes, not a stale first attempt: {:?}",
        diff.left.buffer.content()
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().display_name.as_deref(),
        Some("doc.md: editor <-> disk")
    );
    assert_eq!(
        active_row_count(&db_path, doc_db_id),
        1,
        "exactly the fresh session's row is active"
    );

    session
        .app_mut()
        .db
        .take()
        .expect("store present")
        .shutdown();
}
