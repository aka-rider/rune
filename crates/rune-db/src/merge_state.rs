use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::Error;
use crate::ids::{DocId, ObsId, SessionId};
use crate::inherit::is_session_alive;
use crate::retry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergeRowState {
    Active,
    Completed,
    Abandoned,
}

impl MergeRowState {
    fn as_str(self) -> &'static str {
        match self {
            MergeRowState::Active => "active",
            MergeRowState::Completed => "completed",
            MergeRowState::Abandoned => "abandoned",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeCloseState {
    Completed,
    Abandoned,
}

impl From<MergeCloseState> for MergeRowState {
    fn from(state: MergeCloseState) -> MergeRowState {
        match state {
            MergeCloseState::Completed => MergeRowState::Completed,
            MergeCloseState::Abandoned => MergeRowState::Abandoned,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResumableMerge {
    pub blocks_json: String,
    pub theirs_obs: ObsId,
}

#[derive(Clone, Copy)]
pub(crate) struct MergeOpenArgs<'a> {
    pub doc_id: DocId,
    pub session_id: SessionId,
    pub base_obs: Option<ObsId>,
    pub theirs_obs: ObsId,
    pub marker_content: &'a str,
    pub blocks_json: &'a str,
}

pub(crate) fn merge_open(
    conn: &mut Connection,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    args: MergeOpenArgs<'_>,
    now: SystemTime,
) -> Result<(), Error> {
    let at = crate::session::format_rfc3339_nanos(now);
    retry::with_retry(conn, |tx| {
        for (row_id, session_id) in active_rows_newest_first(tx, args.doc_id)? {
            let stale =
                session_id == args.session_id || !is_session_alive(tx, liveness_check, session_id)?;
            if stale {
                set_state(tx, row_id, MergeRowState::Abandoned)?;
            }
        }
        let marker_hash = crate::blob::put_blob(tx, args.marker_content.as_bytes())?;
        tx.execute(
            "INSERT INTO merges(doc_id, session_id, base_obs, theirs_obs, marker_hash, blocks, state, created_at) \
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                args.doc_id,
                args.session_id,
                args.base_obs,
                args.theirs_obs,
                marker_hash,
                args.blocks_json,
                MergeRowState::Active.as_str(),
                at
            ],
        )?;
        Ok(())
    })
}

pub(crate) fn merge_progress(
    conn: &mut Connection,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    doc_id: DocId,
    session_id: SessionId,
    marker_content: &str,
    blocks_json: &str,
) -> Result<(), Error> {
    retry::with_retry(conn, |tx| {
        let target = match newest_active_owned(tx, doc_id, session_id)? {
            Some(row_id) => Some(row_id),
            None => newest_active_dead(tx, liveness_check, doc_id)?,
        };
        let Some(row_id) = target else {
            return Ok(());
        };
        let marker_hash = crate::blob::put_blob(tx, marker_content.as_bytes())?;
        tx.execute(
            "UPDATE merges SET session_id=?1, marker_hash=?2, blocks=?3 WHERE id=?4",
            params![session_id, marker_hash, blocks_json, row_id],
        )?;
        Ok(())
    })
}

pub(crate) fn merge_close(
    conn: &mut Connection,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    doc_id: DocId,
    session_id: SessionId,
    state: MergeCloseState,
) -> Result<(), Error> {
    retry::with_retry(conn, |tx| {
        let target = match newest_active_owned(tx, doc_id, session_id)? {
            Some(row_id) => Some(row_id),
            None => newest_active_dead(tx, liveness_check, doc_id)?,
        };
        let Some(row_id) = target else {
            return Ok(());
        };
        set_state(tx, row_id, state.into())
    })
}

pub(crate) fn resume_candidate(
    tx: &Transaction<'_>,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    doc_id: DocId,
    recovered_hash: &str,
) -> Result<Option<ResumableMerge>, Error> {
    for (row_id, session_id) in active_rows_newest_first(tx, doc_id)? {
        if is_session_alive(tx, liveness_check, session_id)? {
            continue;
        }
        let (marker_hash, blocks_json, theirs_obs): (String, String, ObsId) = tx.query_row(
            "SELECT marker_hash, blocks, theirs_obs FROM merges WHERE id=?1",
            params![row_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if marker_hash == recovered_hash {
            return Ok(Some(ResumableMerge {
                blocks_json,
                theirs_obs,
            }));
        }
        set_state(tx, row_id, MergeRowState::Abandoned)?;
        return Ok(None);
    }
    Ok(None)
}

fn active_rows_newest_first(
    tx: &Transaction<'_>,
    doc_id: DocId,
) -> Result<Vec<(i64, SessionId)>, Error> {
    let mut stmt = tx.prepare(
        "SELECT id, session_id FROM merges WHERE doc_id=?1 AND state=?2 ORDER BY id DESC",
    )?;
    let rows = stmt
        .query_map(params![doc_id, MergeRowState::Active.as_str()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<Result<Vec<(i64, SessionId)>, rusqlite::Error>>()?;
    Ok(rows)
}

fn newest_active_owned(
    tx: &Transaction<'_>,
    doc_id: DocId,
    session_id: SessionId,
) -> Result<Option<i64>, Error> {
    tx.query_row(
        &format!(
            "SELECT id FROM merges WHERE doc_id=?1 AND session_id=?2 AND state='{}' \
             ORDER BY id DESC LIMIT 1",
            MergeRowState::Active.as_str()
        ),
        params![doc_id, session_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(Error::from)
}

fn newest_active_dead(
    tx: &Transaction<'_>,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    doc_id: DocId,
) -> Result<Option<i64>, Error> {
    for (row_id, session_id) in active_rows_newest_first(tx, doc_id)? {
        if !is_session_alive(tx, liveness_check, session_id)? {
            return Ok(Some(row_id));
        }
    }
    Ok(None)
}

fn set_state(tx: &Transaction<'_>, row_id: i64, state: MergeRowState) -> Result<(), Error> {
    tx.execute(
        "UPDATE merges SET state=?1 WHERE id=?2",
        params![state.as_str(), row_id],
    )?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
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
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
        let doc_id = seed_doc(&conn);
        let theirs = seed_observation(&mut conn, doc_id, session_a);
        open_merge(&mut conn, &alive, doc_id, session_a, theirs, "m", "[]");

        let session_b =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
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
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
        let doc_id = seed_doc(&conn);
        let theirs = seed_observation(&mut conn, doc_id, session_a);
        open_merge(&mut conn, &alive, doc_id, session_a, theirs, "m", "[]");

        let session_b =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
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
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
        let doc_id = seed_doc(&conn);
        let theirs = seed_observation(&mut conn, doc_id, session_a);
        open_merge(&mut conn, &alive, doc_id, session_a, theirs, "m", "[]");

        let session_b =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
        merge_progress(&mut conn, &dead, doc_id, session_b, "m", "[9]").expect("merge_progress");

        assert_eq!(
            row_states(&conn, doc_id),
            vec![(session_b, MergeRowState::Active.as_str().to_string())]
        );
    }

    #[test]
    fn resume_candidate_matches_on_the_recorded_marker_hash_and_stays_active() {
        let mut conn = open();
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
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
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
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
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
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
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
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

        let session_b =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
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
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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
}
