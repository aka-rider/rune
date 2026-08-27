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
#[path = "merge_state_tests.rs"]
mod tests;
