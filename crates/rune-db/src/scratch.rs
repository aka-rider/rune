//! Untitled-draft lifecycle: minting a fresh scratch `documents` row,
//! sweeping empty leftover ones, listing genuinely recoverable ones from a
//! prior session, and reconstructing one's content across a session
//! boundary — the untitled-document counterpart to `load.rs`'s
//! disk-backed cross-session inheritance: an untitled document has no
//! backing file to fall back to at all, so `Load`'s own "seed the anchor
//! from raw disk" escape hatch does not exist here.

use std::time::SystemTime;

use rusqlite::{Connection, params};

use crate::Error;
use crate::doc_kind::DocKind;
use crate::ids::{DocId, SessionId};
use crate::inherit::{is_session_alive, most_recent_session_for_doc};
use crate::retry;
use crate::session::format_rfc3339_nanos;

pub fn create_scratch(
    conn: &mut Connection,
    session_id: SessionId,
    now: SystemTime,
) -> Result<DocId, Error> {
    let at = format_rfc3339_nanos(now);
    retry::with_retry(conn, |tx| {
        tx.execute(
            "INSERT INTO documents(path, kind, created_at, last_seen_at) VALUES('',?1,?2,?2)",
            params![DocKind::Scratch.as_str(), at],
        )?;
        let doc_id = DocId(tx.last_insert_rowid());
        tx.execute(
            "INSERT INTO session_documents(session_id, doc_id) VALUES(?1,?2)",
            params![session_id, doc_id],
        )?;
        Ok(doc_id)
    })
}

pub fn gc_empty_scratch(
    conn: &mut Connection,
    keep_id: i64,
    liveness_check: &dyn Fn(i64, &str) -> bool,
) -> Result<i64, Error> {
    retry::with_retry(conn, |tx| {
        let candidates: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM documents \
                 WHERE path='' AND inode IS NULL AND id!=?1 \
                   AND id NOT IN (SELECT DISTINCT doc_id FROM events) \
                   AND id NOT IN (SELECT DISTINCT doc_id FROM snapshots) \
                   AND id NOT IN (SELECT DISTINCT doc_id FROM observations) \
                   AND id NOT IN (SELECT DISTINCT doc_id FROM merges)",
            )?;
            stmt.query_map(params![keep_id], |r| r.get(0))?
                .collect::<Result<Vec<i64>, _>>()?
        };

        let mut deleted = 0i64;
        for doc_id in candidates {
            let claiming_sessions: Vec<SessionId> = {
                let mut stmt = tx
                    .prepare("SELECT DISTINCT session_id FROM session_documents WHERE doc_id=?1")?;
                stmt.query_map(params![doc_id], |r| r.get::<_, i64>(0).map(SessionId))?
                    .collect::<Result<Vec<SessionId>, _>>()?
            };

            let mut any_alive = false;
            for claiming_session in claiming_sessions {
                if is_session_alive(tx, liveness_check, claiming_session)? {
                    any_alive = true;
                    break;
                }
            }
            if any_alive {
                continue;
            }

            let rows = tx.execute("DELETE FROM documents WHERE id=?1", params![doc_id])?;
            deleted += i64::try_from(rows).unwrap_or(i64::MAX);
        }
        Ok(deleted)
    })
}

/// Lists genuine untitled scratch rows carrying history (events or
/// snapshots) from a prior session — unsaved work the user can recover on
/// the next launch — excluding `exclude_id`. Newest first.
///
/// The `inode IS NULL` filter is load-bearing: `rebind.rs`/`document.rs`
/// also blank `path` on eviction, and those orphaned BOUND rows keep a real
/// inode — without this filter they would surface as fake untitled tabs
/// holding real-file content.
pub fn recoverable_scratch(conn: &Connection, exclude_id: i64) -> Result<Vec<i64>, Error> {
    let mut stmt = conn.prepare(
        "SELECT id FROM documents \
         WHERE path='' AND id!=?1 AND inode IS NULL \
           AND (id IN (SELECT DISTINCT doc_id FROM events) \
             OR id IN (SELECT DISTINCT doc_id FROM snapshots)) \
         ORDER BY id DESC",
    )?;
    let ids = stmt
        .query_map(params![exclude_id], |r| r.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(ids)
}

/// Reconstructs `doc_id`'s content across a session boundary — the
/// untitled-document counterpart to `load.rs`'s cross-session disk
/// inheritance. Composed from [`most_recent_session_for_doc`] (whose session
/// authored the newest row for `doc_id`), [`is_session_alive`] (a still-live
/// session's private, unsaved draft stays private), and
/// [`crate::snapshot::recover_document`] (that session's own reconstruction).
/// This omits a "this session already has its own history" branch: every
/// `doc_id` this is called with is one the CURRENT session has never itself
/// touched (a freshly recovered scratch row at launch), so that branch can
/// never apply here.
///
/// `None` covers both "nothing recorded for this doc, ever" and "the most
/// recent other session is still alive" alike — the caller (bootstrap) skips
/// offering the tab in either case, exactly as it would for a genuinely-new
/// scratch.
pub fn reconstruct_scratch(
    conn: &mut Connection,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    doc_id: DocId,
) -> Result<Option<String>, Error> {
    let other_session_id = retry::with_retry(conn, |tx| most_recent_session_for_doc(tx, doc_id))?;
    let Some(other_session_id) = other_session_id else {
        return Ok(None);
    };
    let alive = retry::with_retry(conn, |tx| {
        is_session_alive(tx, liveness_check, other_session_id)
    })?;
    if alive {
        return Ok(None);
    }
    crate::snapshot::recover_document(conn, other_session_id, doc_id).map(Some)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_core::buffer::AppliedEdit;

    fn open() -> Connection {
        crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(
            &crate::conn::memory_uri(),
        ))
        .expect("open")
    }

    fn always_dead(_pid: i64, _started_at: &str) -> bool {
        false
    }

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
        let doc_id =
            create_scratch(&mut conn, dead_session, SystemTime::now()).expect("create scratch");

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
        assert_eq!(reconstructed.as_deref(), Some("unsaved draft"));
    }

    #[test]
    fn empty_scratch_is_gc_d_but_the_kept_id_and_history_bearing_rows_survive() {
        let mut conn = open();
        let owner_session =
            crate::session::establish_session(&conn, SystemTime::now()).expect("owner session");
        let keep_id = create_scratch(&mut conn, owner_session, SystemTime::now()).expect("keep");
        let empty_id = create_scratch(&mut conn, owner_session, SystemTime::now()).expect("empty");
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let with_history_id =
            create_scratch(&mut conn, session_id, SystemTime::now()).expect("with history");
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
        let draft_id = create_scratch(&mut conn, live_session, SystemTime::now())
            .expect("live session's draft");
        let keep_id = create_scratch(&mut conn, live_session, SystemTime::now()).expect("keep");

        let deleted = gc_empty_scratch(&mut conn, keep_id.0, &always_alive).expect("gc");
        assert_eq!(
            deleted, 0,
            "a draft claimed by a still-running session must never be swept"
        );

        let still_present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
                params![draft_id.0],
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
        let draft_id = create_scratch(&mut conn, dead_session, SystemTime::now())
            .expect("dead session's draft");
        let keep_id = create_scratch(&mut conn, dead_session, SystemTime::now()).expect("keep");

        let deleted = gc_empty_scratch(&mut conn, keep_id.0, &always_dead).expect("gc");
        assert_eq!(deleted, 1);

        let still_present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
                params![draft_id.0],
                |r| r.get(0),
            )
            .expect("check draft row");
        assert!(
            !still_present,
            "a draft whose claiming session is confirmed dead must be swept"
        );
    }

    /// The load-bearing filter: an evicted BOUND row (a real file whose path
    /// was blanked by inode-change eviction, so `path=''` but `inode` is
    /// still set) must be neither offered as recoverable nor swept by GC —
    /// it is a live CAS baseline's row, not a genuine scratch.
    #[test]
    fn evicted_bound_row_is_neither_offered_nor_gc_d() {
        let mut conn = open();
        let at = crate::session::format_rfc3339_nanos(SystemTime::now());
        conn.execute(
            "INSERT INTO documents(path, inode, device, kind, created_at, last_seen_at) \
             VALUES('', 42, 7, 'file', ?1, ?1)",
            params![at],
        )
        .expect("seed evicted-but-bound row");
        let evicted_id = conn.last_insert_rowid();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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

        let keep_id = create_scratch(&mut conn, session_id, SystemTime::now()).expect("keep");
        gc_empty_scratch(&mut conn, keep_id.0, &always_dead).expect("gc");
        let still_present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
                params![evicted_id],
                |r| r.get(0),
            )
            .expect("check evicted row");
        assert!(
            still_present,
            "an evicted bound row's observations must never be GC'd away"
        );
    }

    /// A still-live session's unsaved scratch stays private — its content
    /// is never handed to a fresh session's reconstruction.
    #[test]
    fn reconstruct_scratch_finds_nothing_for_a_still_alive_session() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id =
            create_scratch(&mut conn, session_id, SystemTime::now()).expect("create scratch");
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
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id =
            create_scratch(&mut conn, session_id, SystemTime::now()).expect("create scratch");
        let reconstructed =
            reconstruct_scratch(&mut conn, &always_dead, doc_id).expect("reconstruct_scratch");
        assert_eq!(reconstructed, None);
    }
}
