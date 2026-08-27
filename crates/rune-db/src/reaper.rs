use rusqlite::{Connection, Transaction, params};

use std::time::SystemTime;

use crate::Error;
use crate::ids::{DocId, SessionId};
use crate::observation;
use crate::retry;
use crate::session::parse_rfc3339_nanos;

pub fn reap_dead_sessions(
    conn: &mut Connection,
    is_alive: &dyn Fn(i64, &str) -> bool,
    boot: Option<SystemTime>,
) -> Result<(), Error> {
    let candidates: Vec<(SessionId, i64, String, String)> = {
        let mut stmt = conn.prepare("SELECT id, pid, proc_started_at, opened_at FROM sessions")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for (id, pid, started_at, opened_at) in candidates {
        let dead_since_reboot = started_at.is_empty() && predates_boot(&opened_at, boot);
        if !dead_since_reboot && is_alive(pid, &started_at) {
            continue;
        }
        let reapable = retry::with_retry(conn, |tx| session_is_reapable(tx, id))?;
        if !reapable {
            continue;
        }
        retry::with_retry(conn, |tx| reap_session_footprint(tx, id))?;
    }
    Ok(())
}

fn predates_boot(opened_at: &str, boot: Option<std::time::SystemTime>) -> bool {
    match (parse_rfc3339_nanos(opened_at), boot) {
        (Some(opened), Some(boot)) => opened < boot,
        _ => false,
    }
}

fn session_is_reapable(tx: &Transaction<'_>, session_id: SessionId) -> Result<bool, Error> {
    let doc_ids: Vec<DocId> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT doc_id FROM ( \
                SELECT doc_id FROM events    WHERE session_id=?1 \
                UNION \
                SELECT doc_id FROM snapshots WHERE session_id=?1 \
             )",
        )?;
        let rows = stmt.query_map(params![session_id], |r| r.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for doc_id in doc_ids {
        if doc_footprint_is_unmaterialized(tx, session_id, doc_id)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn doc_footprint_is_unmaterialized(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
) -> Result<bool, Error> {
    let Some(baseline) = observation::saved_obs_for(tx, session_id, doc_id)? else {
        return Ok(true);
    };
    let Some(materialized_seq) = baseline.seq else {
        return Ok(true);
    };
    tx.query_row(
        "SELECT EXISTS( \
            SELECT 1 FROM events    WHERE session_id=?1 AND doc_id=?2 AND seq > ?3 \
            UNION ALL \
            SELECT 1 FROM snapshots WHERE session_id=?1 AND doc_id=?2 AND seq > ?3 \
         )",
        params![session_id, doc_id, materialized_seq],
        |r| r.get(0),
    )
    .map_err(Error::from)
}

fn reap_session_footprint(tx: &Transaction<'_>, session_id: SessionId) -> Result<(), Error> {
    tx.execute(
        "DELETE FROM session_documents WHERE session_id=?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM events WHERE session_id=?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM snapshots WHERE session_id=?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM sessions WHERE id=?1 \
         AND NOT EXISTS(SELECT 1 FROM observations WHERE session_id=?1) \
         AND NOT EXISTS(SELECT 1 FROM merges WHERE session_id=?1)",
        params![session_id],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "reaper_tests.rs"]
mod tests;
