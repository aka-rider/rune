use rusqlite::{Connection, Transaction, params};

use std::time::{Duration, SystemTime};

use crate::Error;
use crate::ids::{DocId, SessionId};
use crate::inherit::most_recent_session_for_doc;
use crate::observation;
use crate::retry;
use crate::session::parse_rfc3339_nanos;

pub(crate) const HISTORY_TTL: Duration = Duration::from_secs(90 * 24 * 60 * 60);

pub fn reap_dead_sessions(
    conn: &mut Connection,
    is_alive: &dyn Fn(i64, &str) -> bool,
    boot: Option<SystemTime>,
    now: SystemTime,
) -> Result<(), Error> {
    let candidates: Vec<(SessionId, i64, String, String)> = {
        let mut stmt = conn.prepare("SELECT id, pid, proc_started_at, opened_at FROM sessions")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut unjudgeable: Vec<String> = Vec::new();
    for (id, pid, started_at, opened_at) in candidates {
        let dead_since_reboot = started_at.is_empty() && predates_boot(&opened_at, boot);
        if !dead_since_reboot && is_alive(pid, &started_at) {
            continue;
        }
        unjudgeable.extend(retry::with_retry(conn, |tx| {
            reap_session_footprint(tx, id, now)
        })?);
    }
    if unjudgeable.is_empty() {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "kept dead-session history that has no usable edit time: {}",
            unjudgeable.join("; ")
        )))
    }
}

fn predates_boot(opened_at: &str, boot: Option<std::time::SystemTime>) -> bool {
    match (parse_rfc3339_nanos(opened_at), boot) {
        (Some(opened), Some(boot)) => opened < boot,
        _ => false,
    }
}

fn footprint_docs(tx: &Transaction<'_>, session_id: SessionId) -> Result<Vec<DocId>, Error> {
    let mut stmt = tx.prepare(
        "SELECT DISTINCT doc_id FROM ( \
            SELECT doc_id FROM events    WHERE session_id=?1 \
            UNION \
            SELECT doc_id FROM snapshots WHERE session_id=?1 \
         )",
    )?;
    let rows = stmt.query_map(params![session_id], |r| r.get(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Error::from)
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
            UNION ALL \
            SELECT 1 FROM session_documents \
                WHERE session_id=?1 AND doc_id=?2 AND current_seq IS NOT NULL AND current_seq != ?3 \
         )",
        params![session_id, doc_id, materialized_seq],
        |r| r.get(0),
    )
    .map_err(Error::from)
}

enum Retention {
    Keep,
    Reap,
    Unjudgeable(String),
}

fn retention(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    now: SystemTime,
) -> Result<Retention, Error> {
    if doc_footprint_is_unmaterialized(tx, session_id, doc_id)? {
        return Ok(Retention::Keep);
    }
    if most_recent_session_for_doc(tx, doc_id)? != Some(session_id) {
        return Ok(Retention::Reap);
    }
    let (newest_edit, last_opened): (Option<String>, String) = tx.query_row(
        "SELECT (SELECT MAX(at) FROM events WHERE session_id=?1 AND doc_id=?2), last_seen_at \
         FROM documents WHERE id=?2",
        params![session_id, doc_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut last_used: Option<SystemTime> = None;
    for stamp in newest_edit.iter().chain([&last_opened]) {
        let Some(used) = parse_rfc3339_nanos(stamp) else {
            return Ok(Retention::Unjudgeable(format!(
                "session {session_id} doc {doc_id} use time {stamp:?}"
            )));
        };
        last_used = last_used.max(Some(used));
    }
    let expired =
        last_used.is_some_and(|used| now.duration_since(used).is_ok_and(|age| age > HISTORY_TTL));
    Ok(if expired {
        Retention::Reap
    } else {
        Retention::Keep
    })
}

fn reap_session_footprint(
    tx: &Transaction<'_>,
    session_id: SessionId,
    now: SystemTime,
) -> Result<Vec<String>, Error> {
    let mut unjudgeable = Vec::new();
    for doc_id in footprint_docs(tx, session_id)? {
        match retention(tx, session_id, doc_id, now)? {
            Retention::Keep => continue,
            Retention::Unjudgeable(why) => {
                unjudgeable.push(why);
                continue;
            }
            Retention::Reap => {}
        }
        tx.execute(
            "DELETE FROM events WHERE session_id=?1 AND doc_id=?2",
            params![session_id, doc_id],
        )?;
        tx.execute(
            "DELETE FROM snapshots WHERE session_id=?1 AND doc_id=?2",
            params![session_id, doc_id],
        )?;
    }
    tx.execute(
        "DELETE FROM session_documents WHERE session_id=?1 \
         AND NOT EXISTS(SELECT 1 FROM events WHERE session_id=?1 AND doc_id=session_documents.doc_id) \
         AND NOT EXISTS(SELECT 1 FROM snapshots WHERE session_id=?1 AND doc_id=session_documents.doc_id)",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM sessions WHERE id=?1 \
         AND NOT EXISTS(SELECT 1 FROM events WHERE session_id=?1) \
         AND NOT EXISTS(SELECT 1 FROM snapshots WHERE session_id=?1) \
         AND NOT EXISTS(SELECT 1 FROM observations WHERE session_id=?1) \
         AND NOT EXISTS(SELECT 1 FROM merges WHERE session_id=?1)",
        params![session_id],
    )?;
    Ok(unjudgeable)
}

#[cfg(test)]
#[path = "reaper_tests.rs"]
mod tests;
