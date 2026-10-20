//! Dead-session history retention: the newest dead lineage of a document
//! survives until `HISTORY_TTL` of inactivity, so the next session can
//! adopt it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::journal_append::EditBatch;
use crate::load::load;
use rune_core::undo::EditKind;
use rune_vfs::{Mem, VfsTestExt};
use std::path::Path;

fn edit_at(conn: &mut Connection, session_id: SessionId, doc_id: DocId, at: SystemTime) {
    let tx = conn.transaction().expect("tx");
    crate::journal::append_edit(
        &tx,
        session_id,
        at,
        doc_id,
        EditBatch {
            edits: &[rune_core::buffer::AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "x".to_string(),
            }],
            cursors_before: &[],
            cursors_after: &[],
            kind: EditKind::Other,
        },
    )
    .expect("append_edit");
    tx.commit().expect("commit");
}

fn count_events(conn: &Connection, session_id: SessionId, doc_id: DocId) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM events WHERE session_id=?1 AND doc_id=?2",
        params![session_id, doc_id],
        |r| r.get(0),
    )
    .expect("count")
}

fn count_session_documents(conn: &Connection, session_id: SessionId, doc_id: DocId) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM session_documents WHERE session_id=?1 AND doc_id=?2",
        params![session_id, doc_id],
        |r| r.get(0),
    )
    .expect("count")
}

fn reap_dead_111(conn: &mut Connection, now: SystemTime) {
    reap_dead_sessions(conn, &|pid, _| pid != 111, None, now).expect("reap");
}

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

#[test]
fn the_newest_dead_history_of_a_document_survives_the_reap() {
    let mut conn = open();
    let dead = seed_session(&conn, 111);
    let doc = seed_doc(&conn);
    edit_at(&mut conn, dead, doc, t0());
    materialize_footprint(&mut conn, dead, doc);

    reap_dead_111(&mut conn, t0() + Duration::from_secs(24 * 3600));

    assert_eq!(count_events(&conn, dead, doc), 1);
    assert_eq!(count_session_documents(&conn, dead, doc), 1);
}

#[test]
fn only_the_documents_a_later_session_superseded_are_reaped() {
    let mut conn = open();
    let dead = seed_session(&conn, 111);
    let kept_doc = seed_doc(&conn);
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/other', 'x', '1970-01-01T00:00:00.000000000Z')",
        [],
    )
    .expect("second doc");
    let superseded_doc = DocId(conn.last_insert_rowid());
    for doc in [kept_doc, superseded_doc] {
        edit_at(&mut conn, dead, doc, t0());
        materialize_footprint(&mut conn, dead, doc);
    }
    let later = seed_session(&conn, 222);
    edit_at(&mut conn, later, superseded_doc, t0());

    reap_dead_111(&mut conn, t0());

    assert_eq!(
        (
            count_events(&conn, dead, kept_doc),
            count_events(&conn, dead, superseded_doc),
            count_session_documents(&conn, dead, superseded_doc),
        ),
        (1, 0, 0)
    );
}

#[test]
fn a_dead_history_expires_once_its_newest_edit_is_older_than_the_ttl() {
    let mut conn = open();
    let dead = seed_session(&conn, 111);
    let doc = seed_doc(&conn);
    edit_at(&mut conn, dead, doc, t0());
    materialize_footprint(&mut conn, dead, doc);

    reap_dead_111(&mut conn, t0() + HISTORY_TTL);
    assert_eq!(
        count_events(&conn, dead, doc),
        1,
        "exactly at the TTL is still retained"
    );

    reap_dead_111(&mut conn, t0() + HISTORY_TTL + Duration::from_secs(1));
    assert_eq!(count_events(&conn, dead, doc), 0);
}

fn dead_session_whose_only_observation_belongs_to_another_session(
    conn: &mut Connection,
) -> (SessionId, DocId) {
    let dead = seed_session(conn, 111);
    let doc = seed_doc(conn);
    edit_at(conn, dead, doc, t0());
    let other = seed_session(conn, 222);
    seed_blob(conn, "baseline");
    conn.execute(
        "INSERT INTO observations(doc_id, session_id, blob_hash, seq, origin, at) \
         VALUES(?1, ?2, 'baseline', (SELECT MAX(seq) FROM events), 'save', 'x')",
        params![doc, other],
    )
    .expect("seed observation");
    let observation = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO session_documents(session_id, doc_id, saved_obs) VALUES(?1, ?2, ?3)",
        params![dead, doc, observation],
    )
    .expect("point baseline at it");
    (dead, doc)
}

#[test]
fn a_sessions_row_survives_while_retained_history_still_hangs_off_it() {
    let mut conn = open();
    let (dead, doc) = dead_session_whose_only_observation_belongs_to_another_session(&mut conn);

    reap_dead_111(&mut conn, t0());

    assert_eq!(count_events(&conn, dead, doc), 1);
    assert!(sessions_row_exists(&conn, dead));
}

#[test]
fn a_sessions_row_goes_with_its_last_expired_history() {
    let mut conn = open();
    let (dead, doc) = dead_session_whose_only_observation_belongs_to_another_session(&mut conn);

    reap_dead_111(&mut conn, t0() + HISTORY_TTL + Duration::from_secs(1));

    assert_eq!(count_events(&conn, dead, doc), 0);
    assert!(!sessions_row_exists(&conn, dead));
}

#[test]
fn a_session_parked_by_undo_below_its_saved_seq_keeps_the_unsaved_text() {
    let mut conn = open();
    let dead = seed_session(&conn, 111);
    let doc = seed_doc(&conn);
    edit_at(&mut conn, dead, doc, t0());
    edit_at(&mut conn, dead, doc, t0());
    materialize_footprint(&mut conn, dead, doc);
    let saved = retry::with_retry(&mut conn, |tx| crate::journal::current_seq(tx, dead, doc))
        .expect("current_seq");
    retry::with_retry(&mut conn, |tx| {
        crate::journal::move_undo_pos(tx, dead, doc, crate::ids::Seq(saved.0 - 1))
    })
    .expect("undo below the saved seq");
    let later = seed_session(&conn, 222);
    edit_at(&mut conn, later, doc, t0());

    reap_dead_111(&mut conn, t0());

    assert_eq!(
        count_events(&conn, dead, doc),
        2,
        "the buffer the user left behind differs from the file, so it is unsaved work"
    );
}

#[test]
fn an_unparsable_edit_time_keeps_the_history_and_surfaces_an_error() {
    let mut conn = open();
    let dead = seed_session(&conn, 111);
    let doc = seed_doc(&conn);
    edit_at(&mut conn, dead, doc, t0());
    materialize_footprint(&mut conn, dead, doc);
    conn.execute("UPDATE events SET at='garbage'", [])
        .expect("corrupt the edit time");

    let reaped = reap_dead_sessions(&mut conn, &|pid, _| pid != 111, None, t0());

    assert!(
        reaped.is_err(),
        "a retention verdict that cannot be made must be reported"
    );
    assert_eq!(count_events(&conn, dead, doc), 1, "and must never delete");
}

#[test]
fn an_unparsable_edit_time_does_not_stop_other_sessions_from_being_reaped() {
    let mut conn = open();
    let broken = seed_session(&conn, 111);
    let broken_doc = seed_doc(&conn);
    edit_at(&mut conn, broken, broken_doc, t0());
    materialize_footprint(&mut conn, broken, broken_doc);
    conn.execute("UPDATE events SET at='garbage'", [])
        .expect("corrupt the edit time");
    let expired = seed_session(&conn, 112);
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/other', 'x', '1970-01-01T00:00:00.000000000Z')",
        [],
    )
    .expect("second doc");
    let expired_doc = DocId(conn.last_insert_rowid());
    edit_at(&mut conn, expired, expired_doc, t0());
    materialize_footprint(&mut conn, expired, expired_doc);

    let reaped = reap_dead_sessions(
        &mut conn,
        &|pid, _| pid != 111 && pid != 112,
        None,
        t0() + HISTORY_TTL + Duration::from_secs(1),
    );

    assert!(reaped.is_err());
    assert_eq!(count_events(&conn, expired, expired_doc), 0);
}

fn second_doc(conn: &Connection) -> DocId {
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/second', 'x', '1970-01-01T00:00:00.000000000Z')",
        [],
    )
    .expect("second doc");
    DocId(conn.last_insert_rowid())
}

#[test]
fn a_document_with_unsaved_edits_does_not_keep_its_sessions_other_documents_from_being_superseded()
{
    let mut conn = open();
    let dead = seed_session(&conn, 111);
    let parked = seed_doc(&conn);
    let superseded = second_doc(&conn);
    for doc in [parked, superseded] {
        edit_at(&mut conn, dead, doc, t0());
        materialize_footprint(&mut conn, dead, doc);
    }
    edit_at(&mut conn, dead, parked, t0());
    let later = seed_session(&conn, 222);
    edit_at(&mut conn, later, superseded, t0());

    reap_dead_111(&mut conn, t0());

    assert_eq!(
        (
            count_events(&conn, dead, parked),
            count_events(&conn, dead, superseded)
        ),
        (2, 0),
        "the unsaved edits stay, the saved and superseded history goes"
    );
    assert!(sessions_row_exists(&conn, dead));
}

#[test]
fn a_document_with_unsaved_edits_does_not_keep_its_sessions_other_documents_from_expiring() {
    let mut conn = open();
    let dead = seed_session(&conn, 111);
    let parked = seed_doc(&conn);
    let expiring = second_doc(&conn);
    for doc in [parked, expiring] {
        edit_at(&mut conn, dead, doc, t0());
        materialize_footprint(&mut conn, dead, doc);
    }
    edit_at(&mut conn, dead, parked, t0());

    reap_dead_111(&mut conn, t0() + HISTORY_TTL + Duration::from_secs(1));

    assert_eq!(
        (
            count_events(&conn, dead, parked),
            count_events(&conn, dead, expiring)
        ),
        (2, 0),
        "unsaved edits never expire; the saved history does"
    );
}

#[test]
fn opening_a_document_without_editing_it_restarts_the_clock_of_its_history() {
    const PATH: &str = "/doc.md";
    let mut conn = open();
    let vfs = Mem::new();
    vfs.save_atomic(Path::new(PATH), b"one").expect("seed disk");
    let written = seed_session(&conn, 111);
    let opened = |conn: &mut Connection, session, now| {
        load(
            conn,
            &vfs,
            session,
            &|pid, _| pid != 111,
            Path::new(PATH),
            now,
        )
        .expect("load")
    };
    let doc = opened(&mut conn, written, t0()).doc_id;
    edit_at(&mut conn, written, doc, t0());
    vfs.save_atomic(Path::new(PATH), b"xone").expect("save");
    let seq = retry::with_retry(&mut conn, |tx| {
        crate::journal::current_seq(tx, written, doc)
    })
    .expect("current_seq")
    .0;
    let hash =
        retry::with_retry(&mut conn, |tx| crate::blob::put_blob(tx, b"xone")).expect("put_blob");
    crate::adopt::record_adoption(
        &mut conn,
        doc,
        written,
        observation::ObservationMeta {
            blob_hash: &hash,
            seq: Some(seq),
            origin: crate::obs_origin::ObsOrigin::Save,
            confirmed: crate::confirmation::Confirmation::Confirmed,
        },
        &observation::StatFacts::default(),
        t0(),
        None,
    )
    .expect("record_adoption");
    let reader = seed_session(&conn, 222);
    let adopting = opened(
        &mut conn,
        reader,
        t0() + Duration::from_secs(99 * 24 * 3600),
    );
    assert_eq!(adopting.recovered.content, "xone");

    reap_dead_sessions(
        &mut conn,
        &|_, _| false,
        None,
        t0() + Duration::from_secs(100 * 24 * 3600),
    )
    .expect("reap");

    assert_eq!(
        count_events(&conn, reader, doc),
        1,
        "history last edited 100 days ago but opened yesterday is kept"
    );
}
