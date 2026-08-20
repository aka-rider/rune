#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::test_support::{always_dead, open};
use rune_core::buffer::AppliedEdit;
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
            &text_insert("unsaved draft"),
            &[],
            &[],
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
            &text_insert("x"),
            &[],
            &[],
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
            &text_insert("real file content"),
            &[],
            &[],
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
            &text_insert("still being edited"),
            &[],
            &[],
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
            &text_insert("typed before the crash"),
            &[],
            &[],
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
            &text_insert("typed before the crash"),
            &[],
            &[],
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
            &text_insert("still being typed"),
            &[],
            &[],
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
