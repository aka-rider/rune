#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::confirmation::Confirmation;
use crate::obs_origin::ObsOrigin;
use crate::observation;
use crate::test_support::open;

fn seed_doc(conn: &Connection) -> DocId {
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    DocId(conn.last_insert_rowid())
}

fn seed_observation(conn: &mut Connection, doc_id: DocId, session_id: SessionId) -> ObsId {
    retry::with_retry(conn, |tx| {
        let hash = crate::blob::put_blob(tx, b"theirs bytes")?;
        observation::record_observation(
            tx,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash,
                seq: None,
                origin: ObsOrigin::Probe,
                confirmed: Confirmation::Confirmed,
            },
            &observation::StatFacts::default(),
            "t",
        )
    })
    .expect("seed observation")
}

fn open_merge(
    conn: &mut Connection,
    liveness: &dyn Fn(i64, &str) -> bool,
    doc_id: DocId,
    session_id: SessionId,
    theirs_obs: ObsId,
    marker: &str,
    blocks: &str,
) {
    merge_open(
        conn,
        liveness,
        MergeOpenArgs {
            doc_id,
            session_id,
            base_obs: None,
            theirs_obs,
            marker_content: marker,
            blocks_json: blocks,
        },
        SystemTime::now(),
    )
    .expect("merge_open");
}

fn row_states(conn: &Connection, doc_id: DocId) -> Vec<(SessionId, String)> {
    let mut stmt = conn
        .prepare("SELECT session_id, state FROM merges WHERE doc_id=?1 ORDER BY id")
        .expect("prepare");
    stmt.query_map(params![doc_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<Vec<(SessionId, String)>, _>>()
        .expect("rows")
}

fn alive(_pid: i64, _started_at: &str) -> bool {
    true
}

fn dead(_pid: i64, _started_at: &str) -> bool {
    false
}

#[test]
fn open_progress_close_round_trip_updates_one_row() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let doc_id = seed_doc(&conn);
    let theirs = seed_observation(&mut conn, doc_id, session_id);

    open_merge(
        &mut conn, &alive, doc_id, session_id, theirs, "markers", "[1]",
    );
    merge_progress(&mut conn, &alive, doc_id, session_id, "markers'", "[2]")
        .expect("merge_progress");

    let (blocks, marker_hash, state): (String, String, String) = conn
        .query_row(
            "SELECT blocks, marker_hash, state FROM merges WHERE doc_id=?1",
            params![doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("row");
    assert_eq!(blocks, "[2]");
    assert_eq!(marker_hash, observation::hash_bytes(b"markers'"));
    assert_eq!(state, MergeRowState::Active.as_str());

    merge_close(
        &mut conn,
        &alive,
        doc_id,
        session_id,
        MergeCloseState::Completed,
    )
    .expect("merge_close");
    assert_eq!(
        row_states(&conn, doc_id),
        vec![(session_id, MergeRowState::Completed.as_str().to_string())]
    );

    merge_close(
        &mut conn,
        &alive,
        doc_id,
        session_id,
        MergeCloseState::Completed,
    )
    .expect("close with no active row is a no-op");
}

#[test]
fn open_flips_a_dead_sessions_stale_active_row_to_abandoned() {
    let mut conn = open();
    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    let doc_id = seed_doc(&conn);
    let theirs = seed_observation(&mut conn, doc_id, session_a);
    open_merge(&mut conn, &alive, doc_id, session_a, theirs, "m", "[]");

    let session_b = crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
    open_merge(&mut conn, &dead, doc_id, session_b, theirs, "m2", "[]");

    assert_eq!(
        row_states(&conn, doc_id),
        vec![
            (session_a, MergeRowState::Abandoned.as_str().to_string()),
            (session_b, MergeRowState::Active.as_str().to_string()),
        ]
    );
}

#[test]
fn open_leaves_a_live_sessions_active_row_untouched() {
    let mut conn = open();
    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    let doc_id = seed_doc(&conn);
    let theirs = seed_observation(&mut conn, doc_id, session_a);
    open_merge(&mut conn, &alive, doc_id, session_a, theirs, "m", "[]");

    let session_b = crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
    open_merge(&mut conn, &alive, doc_id, session_b, theirs, "m2", "[]");

    assert_eq!(
        row_states(&conn, doc_id),
        vec![
            (session_a, MergeRowState::Active.as_str().to_string()),
            (session_b, MergeRowState::Active.as_str().to_string()),
        ]
    );
}

#[test]
fn progress_reowns_a_dead_sessions_active_row() {
    let mut conn = open();
    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    let doc_id = seed_doc(&conn);
    let theirs = seed_observation(&mut conn, doc_id, session_a);
    open_merge(&mut conn, &alive, doc_id, session_a, theirs, "m", "[]");

    let session_b = crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
    merge_progress(&mut conn, &dead, doc_id, session_b, "m", "[9]").expect("merge_progress");

    assert_eq!(
        row_states(&conn, doc_id),
        vec![(session_b, MergeRowState::Active.as_str().to_string())]
    );
}

#[test]
fn resume_candidate_matches_on_the_recorded_marker_hash_and_stays_active() {
    let mut conn = open();
    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    let doc_id = seed_doc(&conn);
    let theirs = seed_observation(&mut conn, doc_id, session_a);
    open_merge(
        &mut conn,
        &alive,
        doc_id,
        session_a,
        theirs,
        "working form",
        "[7]",
    );

    let resumed = retry::with_retry(&mut conn, |tx| {
        resume_candidate(tx, &dead, doc_id, &observation::hash_bytes(b"working form"))
    })
    .expect("resume_candidate");
    assert_eq!(
        resumed,
        Some(ResumableMerge {
            blocks_json: "[7]".to_string(),
            theirs_obs: theirs,
        })
    );
    assert_eq!(
        row_states(&conn, doc_id),
        vec![(session_a, MergeRowState::Active.as_str().to_string())]
    );
}

#[test]
fn resume_candidate_flips_a_mismatching_dead_row_to_abandoned() {
    let mut conn = open();
    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    let doc_id = seed_doc(&conn);
    let theirs = seed_observation(&mut conn, doc_id, session_a);
    open_merge(
        &mut conn,
        &alive,
        doc_id,
        session_a,
        theirs,
        "working form",
        "[7]",
    );

    let resumed = retry::with_retry(&mut conn, |tx| {
        resume_candidate(
            tx,
            &dead,
            doc_id,
            &observation::hash_bytes(b"edited past the install"),
        )
    })
    .expect("resume_candidate");
    assert_eq!(resumed, None);
    assert_eq!(
        row_states(&conn, doc_id),
        vec![(session_a, MergeRowState::Abandoned.as_str().to_string())]
    );
}

#[test]
fn resume_candidate_never_touches_a_live_sessions_row() {
    let mut conn = open();
    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    let doc_id = seed_doc(&conn);
    let theirs = seed_observation(&mut conn, doc_id, session_a);
    open_merge(
        &mut conn,
        &alive,
        doc_id,
        session_a,
        theirs,
        "working form",
        "[7]",
    );

    let resumed = retry::with_retry(&mut conn, |tx| {
        resume_candidate(
            tx,
            &alive,
            doc_id,
            &observation::hash_bytes(b"working form"),
        )
    })
    .expect("resume_candidate");
    assert_eq!(resumed, None);
    assert_eq!(
        row_states(&conn, doc_id),
        vec![(session_a, MergeRowState::Active.as_str().to_string())]
    );
}

#[test]
fn merge_close_closes_a_resumed_but_not_re_owned_row() {
    let mut conn = open();
    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    let doc_id = seed_doc(&conn);
    let theirs = seed_observation(&mut conn, doc_id, session_a);
    open_merge(
        &mut conn,
        &alive,
        doc_id,
        session_a,
        theirs,
        "working form",
        "[7]",
    );

    let session_b = crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
    let resumed = retry::with_retry(&mut conn, |tx| {
        resume_candidate(tx, &dead, doc_id, &observation::hash_bytes(b"working form"))
    })
    .expect("resume_candidate");
    assert!(resumed.is_some(), "test setup: the row must resume");
    assert_eq!(
        row_states(&conn, doc_id),
        vec![(session_a, MergeRowState::Active.as_str().to_string())],
        "test setup: resume_candidate never re-owns the row"
    );

    merge_close(
        &mut conn,
        &dead,
        doc_id,
        session_b,
        MergeCloseState::Completed,
    )
    .expect("merge_close");

    assert_eq!(
        row_states(&conn, doc_id),
        vec![(session_a, MergeRowState::Completed.as_str().to_string())],
        "the resumed row must close even though session_b never re-owned it"
    );
}

#[test]
fn merge_open_is_idempotent_for_the_same_session_and_doc() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let doc_id = seed_doc(&conn);
    let theirs = seed_observation(&mut conn, doc_id, session_id);

    open_merge(&mut conn, &alive, doc_id, session_id, theirs, "m1", "[1]");
    open_merge(&mut conn, &alive, doc_id, session_id, theirs, "m2", "[2]");

    let active_rows: Vec<(SessionId, String)> = row_states(&conn, doc_id)
        .into_iter()
        .filter(|(_, state)| state == MergeRowState::Active.as_str())
        .collect();
    assert_eq!(
        active_rows,
        vec![(session_id, MergeRowState::Active.as_str().to_string())],
        "exactly one active row may survive a second open for the same session+doc"
    );
    let (blocks, state): (String, String) = conn
        .query_row(
            "SELECT blocks, state FROM merges WHERE doc_id=?1 AND state='active'",
            params![doc_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!(blocks, "[2]", "the surviving row must be the newer open");
    assert_eq!(state, MergeRowState::Active.as_str());
}
