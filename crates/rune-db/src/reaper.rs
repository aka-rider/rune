//! The dead-session reaper — ported from Go's liveness layer.
//! Best-effort on `Store::open` (never blocks open — any error here is
//! swallowed by the caller, not surfaced as an open failure): for every
//! `sessions` row confirmed dead, deletes its `session_documents`/`events`/
//! `snapshots` footprint — but ONLY once it is no longer
//! [`crate::load::most_recent_session_for_doc`] for any `doc_id` it ever
//! touched. Reaping the currently-most-recent dead session for a doc would
//! destroy the exact unsaved content the next opener still needs to
//! inherit (`load::find_inheritable_draft`).
//!
//! The `sessions` row itself is deliberately NEVER deleted here:
//! `observations.session_id` has no cascade, by design, since a dead
//! session's own save/load/resolve observation must remain a valid,
//! visible "theirs" fact to every other session forever.

use rusqlite::{Connection, Transaction, params};

use crate::Error;
use crate::load::most_recent_session_for_doc;
use crate::retry;

/// Runs once per `Store::open`. `is_alive` decides whether a recorded
/// `(pid, proc_started_at)` pair still identifies a running process — the
/// caller passes the real liveness check in production, a deterministic
/// stand-in in tests. Port of `liveness.go` (`reapDeadSessions`).
pub fn reap_dead_sessions(
    conn: &mut Connection,
    is_alive: &dyn Fn(i64, &str) -> bool,
) -> Result<(), Error> {
    let candidates: Vec<(i64, i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, pid, proc_started_at FROM sessions")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut v = Vec::new();
        for row in rows {
            v.push(row?);
        }
        v
    };

    for (id, pid, started_at) in candidates {
        if is_alive(pid, &started_at) {
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

/// Reports whether `session_id` is safe to reap: for EVERY `doc_id` it ever
/// touched, some OTHER session must now hold the higher seq. A session that
/// never touched any doc (vacuously true) is reapable. Port of
/// `liveness.go` (`sessionIsReapable`).
fn session_is_reapable(tx: &Transaction<'_>, session_id: i64) -> Result<bool, Error> {
    let doc_ids: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT doc_id FROM ( \
                SELECT doc_id FROM events    WHERE session_id=?1 \
                UNION \
                SELECT doc_id FROM snapshots WHERE session_id=?1 \
             )",
        )?;
        let rows = stmt.query_map(params![session_id], |r| r.get(0))?;
        let mut v = Vec::new();
        for row in rows {
            v.push(row?);
        }
        v
    };

    for doc_id in doc_ids {
        if let Some(most_recent) = most_recent_session_for_doc(tx, doc_id)?
            && most_recent == session_id
        {
            return Ok(false); // still the most-recent toucher of this doc
        }
    }
    Ok(true)
}

/// Deletes `session_id`'s `session_documents`/`events`/`snapshots` rows,
/// leaving the `sessions` row itself in place. Port of `liveness.go`
/// (`reapSessionFootprint`).
fn reap_session_footprint(tx: &Transaction<'_>, session_id: i64) -> Result<(), Error> {
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
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn seed_doc(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        conn.last_insert_rowid()
    }

    /// Inserts a `sessions` row directly with a caller-chosen `pid`, rather
    /// than `session::establish_session` (which always stamps the REAL
    /// current process pid) — every test in this module runs in-process, so
    /// two `establish_session` calls would be indistinguishable to
    /// `is_alive`'s `(pid, started_at)` signature. Fabricated pids let each
    /// test simulate distinct processes deterministically.
    fn seed_session(conn: &Connection, pid: i64) -> i64 {
        conn.execute(
            "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES(?1, 'started', 'opened')",
            params![pid],
        )
        .expect("seed session");
        conn.last_insert_rowid()
    }

    fn journal_one_edit(conn: &mut Connection, session_id: i64, doc_id: i64) {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "x".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit");
        tx.commit().expect("commit");
    }

    /// A dead session that is NOT the most-recent toucher of any doc it
    /// touched has its footprint reaped; the `sessions` row itself
    /// survives.
    #[test]
    fn reaper_deletes_footprint_of_a_superseded_dead_session() {
        let mut conn = open();
        let session_old = seed_session(&conn, 111);
        let doc_id = seed_doc(&conn);
        journal_one_edit(&mut conn, session_old, doc_id);

        // A later session supersedes session_old as the most-recent toucher.
        let session_new = seed_session(&conn, 222);
        journal_one_edit(&mut conn, session_new, doc_id);

        // pid 111 (session_old) is dead; pid 222 (session_new) is alive.
        reap_dead_sessions(&mut conn, &|pid, _| pid != 111).expect("reap");

        let old_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                params![session_old],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            old_events, 0,
            "superseded dead session's footprint must be reaped"
        );

        let sessions_row_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                params![session_old],
                |r| r.get(0),
            )
            .expect("check");
        assert!(
            sessions_row_exists,
            "the sessions row itself must never be deleted"
        );
    }

    /// The reaper must SPARE a dead session that is still the most-recent
    /// toucher of a doc — reaping it would destroy content a future
    /// `find_inheritable_draft` still needs.
    #[test]
    fn reaper_spares_the_most_recent_dead_session() {
        let mut conn = open();
        let session_id = seed_session(&conn, 111);
        let doc_id = seed_doc(&conn);
        journal_one_edit(&mut conn, session_id, doc_id);

        reap_dead_sessions(&mut conn, &|_pid, _started_at| false).expect("reap");

        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                params![session_id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            events, 1,
            "the most-recent dead session's footprint must survive"
        );
    }

    /// An alive session is never touched by the reaper at all.
    #[test]
    fn reaper_never_touches_a_live_session() {
        let mut conn = open();
        let session_id = seed_session(&conn, 111);
        let doc_id = seed_doc(&conn);
        journal_one_edit(&mut conn, session_id, doc_id);
        // A later session supersedes it, but it's still reported alive.
        let session_new = seed_session(&conn, 222);
        journal_one_edit(&mut conn, session_new, doc_id);

        reap_dead_sessions(&mut conn, &|_pid, _started_at| true).expect("reap");

        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                params![session_id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(events, 1, "a live session must never be reaped");
    }
}
