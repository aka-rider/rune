//! Cross-session inheritance: finding a DIFFERENT, now-dead session's
//! unsaved draft for a document a fresh session is loading. Split out of
//! `load.rs` (§1.6) — see that module's doc comment for where this fits
//! in `load`'s overall sequence.

use rusqlite::{OptionalExtension, Transaction, params};

use crate::Error;
use crate::observation;
use crate::retry;

/// The session_id attached to whichever row — across `doc_id`'s events and
/// snapshots together — carries the highest seq. Ties break by higher
/// `session_id`. `None` means `doc_id` has no session-scoped activity
/// recorded at all. Shared by [`find_inheritable_draft`] and
/// `reaper::session_is_reapable`. Port of `load.go`
/// (`mostRecentSessionForDoc`).
pub(crate) fn most_recent_session_for_doc(
    tx: &Transaction<'_>,
    doc_id: i64,
) -> Result<Option<i64>, Error> {
    tx.query_row(
        "SELECT session_id FROM ( \
            SELECT session_id, seq FROM events    WHERE doc_id=?1 \
            UNION ALL \
            SELECT session_id, seq FROM snapshots WHERE doc_id=?1 \
         ) \
         ORDER BY seq DESC, session_id DESC LIMIT 1",
        params![doc_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(Error::from)
}

/// Whether `session_id`'s recorded process is still alive, per
/// `liveness_check`. `false` (not an error) when the `sessions` row itself is
/// gone. Shared by [`find_inheritable_draft`] and `scratch::reconstruct_scratch`
/// (the untitled-document counterpart to this module's disk-backed
/// inheritance) so both read the exact same liveness predicate.
pub(crate) fn is_session_alive(
    tx: &Transaction<'_>,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    session_id: i64,
) -> Result<bool, Error> {
    let row: Option<(i64, String)> = tx
        .query_row(
            "SELECT pid, proc_started_at FROM sessions WHERE id=?1",
            params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((pid, started_at)) = row else {
        return Ok(false); // the session row itself is gone -> unambiguously not alive
    };
    Ok(liveness_check(pid, &started_at))
}

/// Looks for a DIFFERENT, now-confirmed-dead session's unsaved content for
/// `doc_id`. Called ONLY from `load`'s `!has_history` branch, before this
/// session has written anything of its own. Returns `(draft, false)` for
/// disk content unchanged for every "nothing to inherit" case: no other
/// session ever touched this doc, the most recent one is still alive, disk
/// moved since that dead session's own last-known baseline, or its content
/// happens to hash-equal disk anyway. Port of `load.go`
/// (`findInheritableDraft`).
pub(crate) fn find_inheritable_draft(
    conn: &mut rusqlite::Connection,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    doc_id: i64,
    disk_content: &str,
    disk_hash: &str,
) -> Result<(String, bool), Error> {
    let other_session_id = retry::with_retry(conn, |tx| most_recent_session_for_doc(tx, doc_id))?;
    let Some(other_session_id) = other_session_id else {
        return Ok((disk_content.to_string(), false));
    };

    let alive = retry::with_retry(conn, |tx| {
        is_session_alive(tx, liveness_check, other_session_id)
    })?;
    if alive {
        return Ok((disk_content.to_string(), false));
    }

    let other_baseline = retry::with_retry(conn, |tx| {
        observation::saved_obs_for(tx, other_session_id, doc_id)
    })?;
    if let Some(baseline) = &other_baseline
        && baseline.blob_hash != disk_hash
    {
        // Disk moved since the dead session's own last-known fact — not
        // safe to bridge (would silently discard the newer disk content).
        return Ok((disk_content.to_string(), false));
    }

    // Re-verify `other_session_id` is STILL the eligible candidate inside the
    // SAME transaction that reads its snapshot/events: a reap racing between
    // the checks above and here can delete `other_session_id`'s entire
    // footprint (`reaper::reap_session_footprint` clears `events`/
    // `snapshots`), and `recover_document` finding zero rows would
    // reconstruct "" — silently presenting an empty buffer for a document
    // with real content ([rune-db 3]). A raced reap must yield "not
    // eligible", never empty content.
    let recovered_draft = retry::with_retry(conn, |tx| {
        let still_candidate = most_recent_session_for_doc(tx, doc_id)?;
        if still_candidate != Some(other_session_id) {
            return Ok(None);
        }
        crate::snapshot::recover_document(tx, other_session_id, doc_id).map(Some)
    })?;
    let Some(recovered_draft) = recovered_draft else {
        return Ok((disk_content.to_string(), false));
    };
    if observation::hash_bytes(recovered_draft.as_bytes()) == disk_hash {
        return Ok((disk_content.to_string(), false)); // never actually diverged from disk
    }
    Ok((recovered_draft, true))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::time::SystemTime;

    use rune_core::buffer::AppliedEdit;
    use rusqlite::Connection;

    use super::*;
    use crate::adopt;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn always_dead(_pid: i64, _started_at: &str) -> bool {
        false
    }

    /// Dead-session inheritance must bail (fall back to plain disk content)
    /// when disk has moved on since the dead session's own last-known
    /// baseline — bridging stale content would silently discard the newer
    /// disk state (`load.go`'s `findInheritableDraft` review-fix doc
    /// comment). Exercises `find_inheritable_draft` directly: `Vfs`'s
    /// atomic-publish-only contract makes an inode-preserving "external
    /// overwrite" impossible to construct through the public `load` entry
    /// point (a real atomic swap legitimately mints a fresh inode, exactly
    /// like Go's own `OpenPath` — orphaning identity is a pre-existing,
    /// out-of-scope concern, not this test's target), so this seeds the
    /// document/session/observation state directly instead.
    #[test]
    fn dead_session_inheritance_bails_when_disk_hash_moved_on() {
        let mut conn = open();
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();

        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");

        // Session A loaded "session A's content" and typed "UNSAVED " on
        // top of it (unsaved edit), with its own baseline (saved_obs)
        // anchored on what it actually saw at load time.
        {
            let tx = conn.transaction().expect("tx");
            crate::snapshot::create_snapshot(
                &tx,
                session_a,
                SystemTime::now(),
                doc_id,
                "session A's content",
                0,
            )
            .expect("anchor snapshot");
            adopt::record_adoption_tx(
                &tx,
                doc_id,
                session_a,
                observation::ObservationMeta {
                    blob_hash: &observation::hash_bytes(b"session A's content"),
                    seq: Some(0),
                    origin: "load",
                },
                &observation::StatFacts {
                    mtime: "t".to_string(),
                    ..Default::default()
                },
                "t",
            )
            .expect("adopt load");
            crate::journal::append_edit(
                &tx,
                session_a,
                SystemTime::now(),
                doc_id,
                &[AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: "UNSAVED ".to_string(),
                }],
                &[],
                &[],
            )
            .expect("append_edit");
            tx.commit().expect("commit");
        }

        // Disk has since moved on to content session A's own baseline never
        // saw ("disk moved on independently" != "session A's content").
        let disk_content = "disk moved on independently";
        let disk_hash = observation::hash_bytes(disk_content.as_bytes());

        let (draft, inheriting) =
            find_inheritable_draft(&mut conn, &always_dead, doc_id, disk_content, &disk_hash)
                .expect("find_inheritable_draft");

        assert!(!inheriting, "must bail, never bridge stale unsaved edits");
        assert_eq!(draft, disk_content, "must fall back to plain disk content");
    }

    /// The mirror-image control: when disk has NOT moved since the dead
    /// session's own baseline, its genuinely unsaved content IS inherited.
    #[test]
    fn dead_session_inheritance_bridges_when_disk_matches_dead_sessions_baseline() {
        let mut conn = open();
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");

        let disk_content = "shared content";
        let disk_hash = observation::hash_bytes(disk_content.as_bytes());
        {
            let tx = conn.transaction().expect("tx");
            crate::snapshot::create_snapshot(
                &tx,
                session_a,
                SystemTime::now(),
                doc_id,
                disk_content,
                0,
            )
            .expect("anchor snapshot");
            adopt::record_adoption_tx(
                &tx,
                doc_id,
                session_a,
                observation::ObservationMeta {
                    blob_hash: &disk_hash,
                    seq: Some(0),
                    origin: "load",
                },
                &observation::StatFacts {
                    mtime: "t".to_string(),
                    ..Default::default()
                },
                "t",
            )
            .expect("adopt load");
            crate::journal::append_edit(
                &tx,
                session_a,
                SystemTime::now(),
                doc_id,
                &[AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: "UNSAVED ".to_string(),
                }],
                &[],
                &[],
            )
            .expect("append_edit");
            tx.commit().expect("commit");
        }

        let (draft, inheriting) =
            find_inheritable_draft(&mut conn, &always_dead, doc_id, disk_content, &disk_hash)
                .expect("find_inheritable_draft");
        assert!(
            inheriting,
            "disk unchanged since the dead session's baseline -> safe to bridge"
        );
        assert_eq!(draft, "UNSAVED shared content");
    }
}
