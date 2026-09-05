#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::journal_append::EditBatch;
use crate::test_support::{always_dead, open};
use rune_core::buffer::AppliedEdit;
use rune_core::undo::EditKind;
use std::time::SystemTime;

fn always_alive(_pid: i64, _started_at: &str) -> bool {
    true
}

fn text_insert(s: &str) -> Vec<AppliedEdit> {
    vec![AppliedEdit {
        start: 0,
        end: 0,
        deleted: String::new(),
        insert: s.to_string(),
    }]
}

#[test]
fn scratch_with_history_from_a_dead_session_is_recoverable_and_reconstructs() {
    let mut conn = open();
    let dead_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("dead session");
    let doc_id = create_scratch_with_intent(&mut conn, dead_session, SystemTime::now(), None)
        .expect("create scratch");

    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            dead_session,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("unsaved draft"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append edit");
        tx.commit().expect("commit");
    }

    let this_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("this session");

    let ids = recoverable_scratch(&conn, this_session.0).expect("recoverable_scratch");
    assert_eq!(ids, vec![doc_id.0], "the dead session's draft must surface");

    let reconstructed =
        reconstruct_scratch(&mut conn, &always_dead, doc_id).expect("reconstruct_scratch");
    assert_eq!(
        reconstructed.map(|r| r.content).as_deref(),
        Some("unsaved draft")
    );
}

#[test]
fn empty_scratch_is_gc_d_but_the_kept_id_and_history_bearing_rows_survive() {
    let mut conn = open();
    let owner_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("owner session");
    let keep_id = create_scratch_with_intent(&mut conn, owner_session, SystemTime::now(), None)
        .expect("keep");
    let empty_id = create_scratch_with_intent(&mut conn, owner_session, SystemTime::now(), None)
        .expect("empty");
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let with_history_id =
        create_scratch_with_intent(&mut conn, session_id, SystemTime::now(), None)
            .expect("with history");
    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            with_history_id,
            EditBatch {
                edits: &text_insert("x"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append edit");
        tx.commit().expect("commit");
    }

    let deleted = gc_empty_scratch(&mut conn, keep_id.0, &always_dead).expect("gc");
    assert_eq!(deleted, 1, "only the truly empty scratch must be swept");

    let remaining_ids: Vec<i64> = conn
        .prepare("SELECT id FROM documents ORDER BY id")
        .expect("prepare")
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<Result<Vec<i64>, _>>()
        .expect("collect");
    assert!(
        remaining_ids.contains(&keep_id.0),
        "keep_id survives regardless of its own owner's liveness"
    );
    assert!(remaining_ids.contains(&with_history_id.0));
    assert!(!remaining_ids.contains(&empty_id.0));
}

#[test]
fn gc_spares_a_draft_claimed_by_a_live_session() {
    let mut conn = open();
    let live_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("live session");
    let draft_id = create_scratch_with_intent(&mut conn, live_session, SystemTime::now(), None)
        .expect("live session's draft");
    let keep_id =
        create_scratch_with_intent(&mut conn, live_session, SystemTime::now(), None).expect("keep");

    let deleted = gc_empty_scratch(&mut conn, keep_id.0, &always_alive).expect("gc");
    assert_eq!(
        deleted, 0,
        "a draft claimed by a still-running session must never be swept"
    );

    let still_present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            rusqlite::params![draft_id.0],
            |r| r.get(0),
        )
        .expect("check draft row");
    assert!(still_present);
}

#[test]
fn gc_sweeps_a_draft_whose_claiming_session_is_dead() {
    let mut conn = open();
    let dead_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("dead session");
    let draft_id = create_scratch_with_intent(&mut conn, dead_session, SystemTime::now(), None)
        .expect("dead session's draft");
    let keep_id =
        create_scratch_with_intent(&mut conn, dead_session, SystemTime::now(), None).expect("keep");

    let deleted = gc_empty_scratch(&mut conn, keep_id.0, &always_dead).expect("gc");
    assert_eq!(deleted, 1);

    let still_present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            rusqlite::params![draft_id.0],
            |r| r.get(0),
        )
        .expect("check draft row");
    assert!(
        !still_present,
        "a draft whose claiming session is confirmed dead must be swept"
    );
}

#[test]
fn evicted_bound_row_is_neither_offered_nor_gc_d() {
    let mut conn = open();
    let at = crate::session::format_rfc3339_nanos(SystemTime::now());
    conn.execute(
        "INSERT INTO documents(path, inode, device, kind, created_at, last_seen_at) \
         VALUES('', 42, 7, 'file', ?1, ?1)",
        rusqlite::params![at],
    )
    .expect("seed evicted-but-bound row");
    let evicted_id = conn.last_insert_rowid();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            DocId(evicted_id),
            EditBatch {
                edits: &text_insert("real file content"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append edit");
        tx.commit().expect("commit");
    }

    let ids = recoverable_scratch(&conn, 0).expect("recoverable_scratch");
    assert!(
        !ids.contains(&evicted_id),
        "an evicted bound row must never be offered as a recoverable draft"
    );

    let keep_id =
        create_scratch_with_intent(&mut conn, session_id, SystemTime::now(), None).expect("keep");
    gc_empty_scratch(&mut conn, keep_id.0, &always_dead).expect("gc");
    let still_present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            rusqlite::params![evicted_id],
            |r| r.get(0),
        )
        .expect("check evicted row");
    assert!(
        still_present,
        "an evicted bound row's observations must never be GC'd away"
    );
}

#[test]
fn reconstruct_scratch_finds_nothing_for_a_still_alive_session() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let doc_id = create_scratch_with_intent(&mut conn, session_id, SystemTime::now(), None)
        .expect("create scratch");
    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("still being edited"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append edit");
        tx.commit().expect("commit");
    }

    let reconstructed =
        reconstruct_scratch(&mut conn, &always_alive, doc_id).expect("reconstruct_scratch");
    assert_eq!(reconstructed, None);
}

#[test]
fn reconstruct_scratch_finds_nothing_for_a_brand_new_scratch() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let doc_id = create_scratch_with_intent(&mut conn, session_id, SystemTime::now(), None)
        .expect("create scratch");
    let reconstructed =
        reconstruct_scratch(&mut conn, &always_dead, doc_id).expect("reconstruct_scratch");
    assert_eq!(reconstructed, None);
}

#[test]
fn find_named_scratch_surfaces_a_dead_sessions_named_draft() {
    let mut conn = open();
    let dead_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("dead session");
    let doc_id = create_scratch_with_intent(
        &mut conn,
        dead_session,
        SystemTime::now(),
        Some("/vault/notes.md"),
    )
    .expect("create named scratch");
    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            dead_session,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("typed before the crash"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append edit");
        tx.commit().expect("commit");
    }

    let ids = find_named_scratch(&conn, "/vault/notes.md").expect("find_named_scratch");
    assert_eq!(ids, vec![doc_id.0]);

    let reconstructed = reconstruct_scratch(&mut conn, &always_dead, doc_id)
        .expect("reconstruct_scratch")
        .expect("must reconstruct the dead session's typed content");
    assert_eq!(reconstructed.content, "typed before the crash");
}

#[test]
fn find_named_scratch_ignores_a_different_intended_path() {
    let mut conn = open();
    let dead_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("dead session");
    let doc_id = create_scratch_with_intent(
        &mut conn,
        dead_session,
        SystemTime::now(),
        Some("/vault/notes.md"),
    )
    .expect("create named scratch");
    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            dead_session,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("typed before the crash"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append edit");
        tx.commit().expect("commit");
    }

    let ids = find_named_scratch(&conn, "/vault/other.md").expect("find_named_scratch");
    assert!(ids.is_empty());
}

#[test]
fn find_named_scratch_lists_a_live_sessions_row_but_reconstruct_refuses_to_steal_it() {
    let mut conn = open();
    let live_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("live session");
    let doc_id = create_scratch_with_intent(
        &mut conn,
        live_session,
        SystemTime::now(),
        Some("/vault/notes.md"),
    )
    .expect("create named scratch");
    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            live_session,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("still being typed"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append edit");
        tx.commit().expect("commit");
    }

    let ids = find_named_scratch(&conn, "/vault/notes.md").expect("find_named_scratch");
    assert_eq!(ids, vec![doc_id.0]);

    let reconstructed =
        reconstruct_scratch(&mut conn, &always_alive, doc_id).expect("reconstruct_scratch");
    assert_eq!(
        reconstructed, None,
        "a live session's own unsaved draft must never be handed to a concurrent launch"
    );
}

#[test]
fn reconstruct_scratch_refuses_an_empty_draft_when_the_candidates_footprint_has_vanished() {
    let mut conn = open();
    let dead_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("dead session");
    let doc_id = create_scratch_with_intent(&mut conn, dead_session, SystemTime::now(), None)
        .expect("create scratch");
    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            dead_session,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("unsaved draft"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append edit");
        tx.commit().expect("commit");
    }

    conn.execute(
        "DELETE FROM events WHERE session_id=?1",
        rusqlite::params![dead_session],
    )
    .expect("simulate a reap racing between the candidate check and the recovery read");
    conn.execute(
        "DELETE FROM snapshots WHERE session_id=?1",
        rusqlite::params![dead_session],
    )
    .expect("simulate a reap racing between the candidate check and the recovery read");

    let unguarded_recovery =
        crate::snapshot::recover_document(&conn, dead_session, doc_id).expect("recover_document");
    assert_eq!(
        unguarded_recovery.content, "",
        "test setup: recover_document alone, given a vanished footprint, reconstructs empty — \
         exactly what the guard below must never surface"
    );

    let reconstructed =
        reconstruct_scratch(&mut conn, &always_dead, doc_id).expect("reconstruct_scratch");
    assert_eq!(
        reconstructed, None,
        "a candidate whose footprint vanished must never surface as an empty draft"
    );
}

#[test]
fn forget_scratch_removes_a_row_with_events_and_snapshots_and_its_history() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let doc_id = create_scratch_with_intent(&mut conn, session_id, SystemTime::now(), None)
        .expect("create scratch");
    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("closed draft"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append edit");
        let seq = crate::journal::current_seq(&tx, session_id, doc_id).expect("current_seq");
        crate::snapshot::create_snapshot(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            "closed draft",
            seq,
        )
        .expect("create snapshot");
        tx.commit().expect("commit");
    }

    let outcome =
        forget_scratch(&mut conn, session_id, doc_id, &always_dead).expect("forget_scratch");
    assert_eq!(outcome, ForgetOutcome::Forgotten);

    let still_present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            rusqlite::params![doc_id.0],
            |r| r.get(0),
        )
        .expect("check document row");
    assert!(!still_present, "forgotten row must be gone");

    let events_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE doc_id=?1",
            rusqlite::params![doc_id.0],
            |r| r.get(0),
        )
        .expect("count events");
    assert_eq!(events_left, 0, "events must cascade away with the row");

    let snapshots_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM snapshots WHERE doc_id=?1",
            rusqlite::params![doc_id.0],
            |r| r.get(0),
        )
        .expect("count snapshots");
    assert_eq!(
        snapshots_left, 0,
        "snapshots must cascade away with the row"
    );

    let other_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("other session");
    let ids = recoverable_scratch(&conn, other_session.0).expect("recoverable_scratch");
    assert!(
        !ids.contains(&doc_id.0),
        "a forgotten row must never surface as recoverable"
    );
}

#[test]
fn forget_scratch_refuses_a_bound_row_with_a_nonempty_path() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let at = crate::session::format_rfc3339_nanos(SystemTime::now());
    conn.execute(
        "INSERT INTO documents(path, kind, created_at, last_seen_at) VALUES('/vault/notes.md', 'file', ?1, ?1)",
        rusqlite::params![at],
    )
    .expect("seed bound row");
    let bound_id = DocId(conn.last_insert_rowid());

    let outcome =
        forget_scratch(&mut conn, session_id, bound_id, &always_dead).expect("forget_scratch");
    assert_eq!(outcome, ForgetOutcome::NotScratch);

    let still_present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            rusqlite::params![bound_id.0],
            |r| r.get(0),
        )
        .expect("check document row");
    assert!(still_present, "a bound row must survive untouched");
}

#[test]
fn forget_scratch_refuses_a_row_with_a_non_null_inode() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let at = crate::session::format_rfc3339_nanos(SystemTime::now());
    conn.execute(
        "INSERT INTO documents(path, inode, device, kind, created_at, last_seen_at) VALUES('', 42, 7, 'file', ?1, ?1)",
        rusqlite::params![at],
    )
    .expect("seed evicted-but-bound row");
    let evicted_id = DocId(conn.last_insert_rowid());

    let outcome =
        forget_scratch(&mut conn, session_id, evicted_id, &always_dead).expect("forget_scratch");
    assert_eq!(outcome, ForgetOutcome::NotScratch);

    let still_present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            rusqlite::params![evicted_id.0],
            |r| r.get(0),
        )
        .expect("check document row");
    assert!(still_present, "an evicted bound row must survive untouched");
}

#[test]
fn forget_scratch_spares_a_row_claimed_by_another_live_session_then_forgets_it_once_dead() {
    let mut conn = open();
    let claiming_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("claiming session");
    let doc_id = create_scratch_with_intent(&mut conn, claiming_session, SystemTime::now(), None)
        .expect("create scratch");
    let own_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("own session");

    let outcome =
        forget_scratch(&mut conn, own_session, doc_id, &always_alive).expect("forget_scratch");
    assert_eq!(outcome, ForgetOutcome::ClaimedByLiveSession);

    let still_present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            rusqlite::params![doc_id.0],
            |r| r.get(0),
        )
        .expect("check document row");
    assert!(
        still_present,
        "a row claimed by another live session must survive"
    );

    let outcome =
        forget_scratch(&mut conn, own_session, doc_id, &always_dead).expect("forget_scratch");
    assert_eq!(
        outcome,
        ForgetOutcome::Forgotten,
        "once the claiming session is confirmed dead, the row is fair game"
    );
}

#[test]
fn forget_scratch_ignores_the_caller_s_own_claim() {
    let mut conn = open();
    let own_session =
        crate::session::establish_session(&conn, SystemTime::now()).expect("own session");
    let doc_id = create_scratch_with_intent(&mut conn, own_session, SystemTime::now(), None)
        .expect("create scratch");

    let liveness_check_called = std::sync::atomic::AtomicBool::new(false);
    let outcome = forget_scratch(&mut conn, own_session, doc_id, &|_pid, _started_at| {
        liveness_check_called.store(true, std::sync::atomic::Ordering::SeqCst);
        true
    })
    .expect("forget_scratch");
    assert!(
        !liveness_check_called.load(std::sync::atomic::Ordering::SeqCst),
        "liveness_check must never be consulted for the caller's own session"
    );
    assert_eq!(
        outcome,
        ForgetOutcome::Forgotten,
        "the caller's own claim on the row must never block forgetting it"
    );
}
