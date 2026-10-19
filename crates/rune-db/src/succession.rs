use rusqlite::{Connection, Transaction, params};

use crate::Error;
use crate::history;
use crate::ids::{DocId, SessionId};
use crate::inherit::{is_session_alive, most_recent_session_for_doc};
use crate::observation;
use crate::retry;
use crate::snapshot;

pub(crate) fn adopt_dead_lineage(
    conn: &mut Connection,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    session_id: SessionId,
    doc_id: DocId,
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
        let shown = snapshot::recover_document(tx, dead, doc_id)?.content;
        if !history::replays(tx, dead, doc_id, &shown)? {
            return Ok(false);
        }
        transfer(tx, dead, session_id, doc_id)?;
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
