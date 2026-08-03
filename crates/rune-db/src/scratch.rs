//! Untitled-draft lifecycle: minting a fresh scratch `documents` row,
//! sweeping empty leftover ones, listing genuinely recoverable ones from a
//! prior session, and reconstructing one's content across a session
//! boundary. Port of `store_documents.go`'s `CreateScratch`/
//! `GCEmptyScratch`/`RecoverableScratch` and `load.go`'s
//! `RecoverAcrossSessions` — the untitled-document counterpart to `load.rs`'s
//! disk-backed cross-session inheritance: an untitled document has no
//! backing file to fall back to at all, so `Load`'s own "seed the anchor
//! from raw disk" escape hatch does not exist here.

use std::time::SystemTime;

use rusqlite::{Connection, params};

use crate::Error;
use crate::inherit::{is_session_alive, most_recent_session_for_doc};
use crate::retry;
use crate::session::format_rfc3339_nanos;

/// Inserts a brand-new, unbound scratch `documents` row and returns its id.
/// inode/device are left NULL (§1.7 — never a literal 0 sentinel): a scratch
/// document has no file identity at all. `schema.rs`'s `kind` CHECK already
/// permits `'scratch'`, and both unique indexes are partial (`WHERE path !=
/// ''`, `WHERE inode IS NOT NULL`), so many `path=''` rows are legal side by
/// side. Port of `store_documents.go` (`CreateScratch`).
pub fn create_scratch(conn: &mut Connection, now: SystemTime) -> Result<i64, Error> {
    let at = format_rfc3339_nanos(now);
    retry::with_retry(conn, |tx| {
        tx.execute(
            "INSERT INTO documents(path, kind, created_at, last_seen_at) VALUES('','scratch',?1,?1)",
            params![at],
        )?;
        Ok(tx.last_insert_rowid())
    })
}

/// Deletes unbound scratch rows carrying neither events nor snapshots —
/// leftover empty drafts from prior sessions. `keep_id` is never deleted
/// (the live untitled document this session is about to bind). Returns the
/// number of rows removed.
///
/// Deliberately STRICTER than Go's `GCEmptyScratch`, which omits the `inode
/// IS NULL` filter: `rebind.rs`/`document.rs` also blank `path` on eviction,
/// and those orphaned BOUND rows retain a real inode. Without this filter
/// this would delete evicted-but-bound rows too and cascade away their
/// `observations` — the CAS-baseline material `sync`/`materialize` derive
/// from — a data-loss bug, not a cosmetic one.
pub fn gc_empty_scratch(conn: &mut Connection, keep_id: i64) -> Result<i64, Error> {
    retry::with_retry(conn, |tx| {
        let deleted = tx.execute(
            "DELETE FROM documents \
             WHERE path='' AND inode IS NULL AND id!=?1 \
               AND id NOT IN (SELECT DISTINCT doc_id FROM events) \
               AND id NOT IN (SELECT DISTINCT doc_id FROM snapshots)",
            params![keep_id],
        )?;
        Ok(i64::try_from(deleted).unwrap_or(i64::MAX))
    })
}

/// Lists genuine untitled scratch rows carrying history (events or
/// snapshots) from a prior session — unsaved work the user can recover on
/// the next launch — excluding `exclude_id`. Newest first. Port of
/// `store_documents.go` (`RecoverableScratch`).
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
/// [`crate::snapshot::recover_document`] (that session's own reconstruction)
/// — Go's `RecoverAcrossSessions`, minus its "this session already has its
/// own history" first branch: every `doc_id` this is called with is one the
/// CURRENT session has never itself touched (a freshly recovered scratch row
/// at launch), so that branch can never apply here.
///
/// `None` covers both "nothing recorded for this doc, ever" and "the most
/// recent other session is still alive" alike — the caller (bootstrap) skips
/// offering the tab in either case, exactly as it would for a genuinely-new
/// scratch.
pub fn reconstruct_scratch(
    conn: &mut Connection,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    doc_id: i64,
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
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
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

    /// End-to-end done-when gate: create a scratch, journal an edit under a
    /// session, mark that session dead, and confirm `recoverable_scratch`
    /// surfaces it while `reconstruct_scratch` yields the draft text.
    #[test]
    fn scratch_with_history_from_a_dead_session_is_recoverable_and_reconstructs() {
        let mut conn = open();
        let dead_session =
            crate::session::establish_session(&conn, SystemTime::now()).expect("dead session");
        let doc_id = create_scratch(&mut conn, SystemTime::now()).expect("create scratch");

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

        // A different, live session is the one calling — excludes its own
        // (irrelevant) id, and never touched doc_id itself.
        let this_session =
            crate::session::establish_session(&conn, SystemTime::now()).expect("this session");

        let ids = recoverable_scratch(&conn, this_session).expect("recoverable_scratch");
        assert_eq!(ids, vec![doc_id], "the dead session's draft must surface");

        let reconstructed =
            reconstruct_scratch(&mut conn, &always_dead, doc_id).expect("reconstruct_scratch");
        assert_eq!(reconstructed.as_deref(), Some("unsaved draft"));
    }

    #[test]
    fn empty_scratch_is_gc_d_but_the_kept_id_and_history_bearing_rows_survive() {
        let mut conn = open();
        let keep_id = create_scratch(&mut conn, SystemTime::now()).expect("keep");
        let empty_id = create_scratch(&mut conn, SystemTime::now()).expect("empty");
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let with_history_id = create_scratch(&mut conn, SystemTime::now()).expect("with history");
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

        let deleted = gc_empty_scratch(&mut conn, keep_id).expect("gc");
        assert_eq!(deleted, 1, "only the truly empty scratch must be swept");

        let remaining_ids: Vec<i64> = conn
            .prepare("SELECT id FROM documents ORDER BY id")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<Vec<i64>, _>>()
            .expect("collect");
        assert!(remaining_ids.contains(&keep_id));
        assert!(remaining_ids.contains(&with_history_id));
        assert!(!remaining_ids.contains(&empty_id));
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
                evicted_id,
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

        let keep_id = create_scratch(&mut conn, SystemTime::now()).expect("keep");
        gc_empty_scratch(&mut conn, keep_id).expect("gc");
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
        let doc_id = create_scratch(&mut conn, SystemTime::now()).expect("create scratch");
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
        let doc_id = create_scratch(&mut conn, SystemTime::now()).expect("create scratch");
        let reconstructed =
            reconstruct_scratch(&mut conn, &always_dead, doc_id).expect("reconstruct_scratch");
        assert_eq!(reconstructed, None);
    }
}
