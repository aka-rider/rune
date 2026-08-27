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
use crate::snapshot::Recovered;

/// Mints a brand-new unbound scratch `documents` row. `intended_path` is
/// `Some` when this scratch names a launch positional that does not exist
/// on disk yet — recorded so a LATER launch of the same path (this session
/// having died before ever materializing) can find its way back to this
/// exact row instead of starting over from an empty buffer — and `None` for
/// the plain bare-launch/quit-guard shape. `intended_path`'s value plays no
/// role in `path=''`'s own scratch-vs-bound meaning anywhere else in this
/// crate: every existing scratch predicate (`recoverable_scratch`,
/// `gc_empty_scratch`'s candidate filter, the evicted-bound-row guard) reads
/// `path`/`inode`, never this column.
pub fn create_scratch_with_intent(
    conn: &mut Connection,
    session_id: SessionId,
    now: SystemTime,
    intended_path: Option<&str>,
) -> Result<DocId, Error> {
    let at = format_rfc3339_nanos(now);
    retry::with_retry(conn, |tx| {
        tx.execute(
            "INSERT INTO documents(path, kind, intended_path, created_at, last_seen_at) VALUES('',?1,?2,?3,?3)",
            params![DocKind::Scratch.as_str(), intended_path, at],
        )?;
        let doc_id = DocId(tx.last_insert_rowid());
        tx.execute(
            "INSERT INTO session_documents(session_id, doc_id) VALUES(?1,?2)",
            params![session_id, doc_id],
        )?;
        Ok(doc_id)
    })
}

/// Lists scratch rows (`path=''`, never bound, `inode IS NULL` — the same
/// evicted-bound-row exclusion [`recoverable_scratch`] uses) recorded as
/// INTENDING `intended_path`, newest first, restricted to rows that
/// actually carry history the way [`recoverable_scratch`] is — a scratch
/// row nobody ever typed into has nothing worth adopting.
///
/// This alone does not decide adoption: the caller still runs each
/// candidate through [`reconstruct_scratch`], which is what actually
/// enforces "a live session's draft is never stolen" (its own
/// [`crate::inherit::is_session_alive`] check) — this only narrows the
/// candidate set by name.
pub fn find_named_scratch(conn: &Connection, intended_path: &str) -> Result<Vec<i64>, Error> {
    let mut stmt = conn.prepare(
        "SELECT id FROM documents \
         WHERE path='' AND inode IS NULL AND intended_path=?1 \
           AND (id IN (SELECT DISTINCT doc_id FROM events) \
             OR id IN (SELECT DISTINCT doc_id FROM snapshots)) \
         ORDER BY id DESC",
    )?;
    let ids = stmt
        .query_map(params![intended_path], |r| r.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(ids)
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
) -> Result<Option<Recovered>, Error> {
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
    retry::with_retry(conn, |tx| {
        let still_candidate = most_recent_session_for_doc(tx, doc_id)?;
        if still_candidate != Some(other_session_id) {
            return Ok(None);
        }
        crate::snapshot::recover_document(tx, other_session_id, doc_id).map(Some)
    })
}

#[cfg(test)]
#[path = "scratch_tests.rs"]
mod tests;
