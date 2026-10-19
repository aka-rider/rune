use std::time::SystemTime;

use rusqlite::{Connection, Transaction, params};

use crate::Error;
use crate::history;
use crate::ids::{DocId, SessionId};
use crate::inherit::{is_session_alive, most_recent_session_for_doc};
use crate::observation;
use crate::retry;
use crate::snapshot;

/// Takes over the most recent dead session's lineage for `doc_id`, trimmed
/// to the history cap. Declines (`Ok(false)`) when there is nothing to adopt
/// or the stored history is damaged, so the caller falls back to inheriting
/// the dead session's draft as a single step. Trimming is safe here and
/// nowhere later: this is the one moment before `session_id` has any
/// history for `doc_id`, hence before any binding can number undo positions
/// against it.
pub(crate) fn adopt_dead_lineage(
    conn: &mut Connection,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    session_id: SessionId,
    doc_id: DocId,
    now: SystemTime,
) -> Result<bool, Error> {
    retry::with_retry(conn, |tx| {
        let Some(dead) = most_recent_session_for_doc(tx, doc_id)? else {
            return Ok(false);
        };
        if is_session_alive(tx, liveness_check, dead)? {
            return Ok(false);
        }
        if observation::saved_obs_for(tx, dead, doc_id)?.is_none() {
            return Ok(false);
        }
        let shown = match snapshot::recover_document(tx, dead, doc_id) {
            Ok(recovered) => recovered.content,
            Err(e) if e.is_stored_data_damage() => return Ok(false),
            Err(e) => return Err(e),
        };
        let replay = history::replay(tx, dead, doc_id, &shown)?;
        if replay.loss.is_some() {
            return Ok(false);
        }
        transfer(tx, dead, session_id, doc_id)?;
        history::trim_to_cap(tx, session_id, doc_id, &replay, now)?;
        Ok(true)
    })
}
fn transfer(
    tx: &Transaction<'_>,
    from: SessionId,
    to: SessionId,
    doc_id: DocId,
) -> Result<(), Error> {
    tx.execute(
        "UPDATE events SET session_id=?1 WHERE session_id=?2 AND doc_id=?3",
        params![to, from, doc_id],
    )?;
    tx.execute(
        "UPDATE snapshots SET session_id=?1 WHERE session_id=?2 AND doc_id=?3",
        params![to, from, doc_id],
    )?;
    tx.execute(
        "UPDATE observations SET session_id=?1 WHERE session_id=?2 AND doc_id=?3",
        params![to, from, doc_id],
    )?;
    tx.execute(
        "UPDATE OR REPLACE session_documents SET session_id=?1 WHERE session_id=?2 AND doc_id=?3",
        params![to, from, doc_id],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "succession_tests.rs"]
mod tests;
