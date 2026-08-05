//! Cross-session inheritance: finding a DIFFERENT, now-dead session's
//! unsaved draft for a document a fresh session is loading. Split out of
//! `load.rs` — see that module's doc comment for where this fits
//! in `load`'s overall sequence.

use rusqlite::{OptionalExtension, Transaction, params};

use crate::Error;
use crate::observation::{self, Observation};
use crate::retry;

/// The session_id attached to whichever row — across `doc_id`'s events and
/// snapshots together — carries the highest seq. Ties break by higher
/// `session_id`. `None` means `doc_id` has no session-scoped activity
/// recorded at all. Shared by [`find_inheritable_draft`] and
/// `reaper::session_is_reapable`.
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

/// The outcome of [`find_inheritable_draft`]: a three-way result, widened
/// from a `(String, bool)` pair so a baseline-hash mismatch no longer has
/// to mean "bail to plain disk content" — it instead carries the dead
/// session's own baseline observation forward so `load` can re-anchor on
/// it.
pub(crate) enum Inherited {
    /// Nothing to inherit: no other session ever touched this doc, the most
    /// recent one is still alive, a reap raced the lookup, or the dead
    /// session's draft happens to hash-equal disk anyway.
    Disk,
    /// The dead session's baseline agrees with what's on disk right now (or
    /// it never recorded one) — safe to bridge disk content straight to
    /// `draft`, exactly like today's anchor-on-disk flow.
    Bridged { draft: String },
    /// The dead session's own baseline (`H0`) no longer matches disk (`H1`)
    /// — disk moved on independently. `draft` is still genuinely unsaved
    /// content worth keeping, but it must be bridged from `H0`, not `H1`, so
    /// the newer disk content is never silently discarded.
    Diverged {
        draft: String,
        baseline: Box<Observation>,
    },
}

/// Looks for a DIFFERENT, now-confirmed-dead session's unsaved content for
/// `doc_id`. Called ONLY from `load`'s `!has_history` branch, before this
/// session has written anything of its own.
pub(crate) fn find_inheritable_draft(
    conn: &mut rusqlite::Connection,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    doc_id: i64,
    disk_hash: &str,
) -> Result<Inherited, Error> {
    let other_session_id = retry::with_retry(conn, |tx| most_recent_session_for_doc(tx, doc_id))?;
    let Some(other_session_id) = other_session_id else {
        return Ok(Inherited::Disk);
    };

    let alive = retry::with_retry(conn, |tx| {
        is_session_alive(tx, liveness_check, other_session_id)
    })?;
    if alive {
        return Ok(Inherited::Disk);
    }

    let other_baseline = retry::with_retry(conn, |tx| {
        observation::saved_obs_for(tx, other_session_id, doc_id)
    })?;

    // Re-verify `other_session_id` is STILL the eligible candidate inside the
    // SAME transaction that reads its snapshot/events: a reap racing between
    // the checks above and here can delete `other_session_id`'s entire
    // footprint (`reaper::reap_session_footprint` clears `events`/
    // `snapshots`), and `recover_document` finding zero rows would
    // reconstruct "" — silently presenting an empty buffer for a document
    // with real content ([rune-db 3]). A raced reap must yield "not
    // eligible", never empty content — this guard runs regardless of
    // whether the baseline below turns out to match disk.
    let recovered_draft = retry::with_retry(conn, |tx| {
        let still_candidate = most_recent_session_for_doc(tx, doc_id)?;
        if still_candidate != Some(other_session_id) {
            return Ok(None);
        }
        crate::snapshot::recover_document(tx, other_session_id, doc_id).map(Some)
    })?;
    let Some(recovered_draft) = recovered_draft else {
        return Ok(Inherited::Disk);
    };
    if observation::hash_bytes(recovered_draft.as_bytes()) == disk_hash {
        return Ok(Inherited::Disk); // never actually diverged from disk
    }

    match other_baseline {
        Some(baseline) if baseline.blob_hash != disk_hash => Ok(Inherited::Diverged {
            draft: recovered_draft,
            baseline: Box::new(baseline),
        }),
        _ => Ok(Inherited::Bridged {
            draft: recovered_draft,
        }),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
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

    /// Dead-session inheritance must carry the dead session's OWN baseline
    /// (`H0`) forward, not bail, when disk has moved on since it (`H1`) —
    /// `load` re-anchors on `H0` and bridges from there, never silently
    /// discarding either the newer disk content or the dead session's
    /// unsaved edit. Exercises `find_inheritable_draft` directly: `Vfs`'s
    /// atomic-publish-only contract makes an inode-preserving "external
    /// overwrite" impossible to construct through the public `load` entry
    /// point, so this seeds the document/session/observation state
    /// directly instead.
    #[test]
    fn dead_session_inheritance_diverges_when_disk_hash_moved_on() {
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

        let inherited = find_inheritable_draft(&mut conn, &always_dead, doc_id, &disk_hash)
            .expect("find_inheritable_draft");

        match inherited {
            Inherited::Diverged { draft, baseline } => {
                assert_eq!(draft, "UNSAVED session A's content");
                assert_eq!(
                    baseline.blob_hash,
                    observation::hash_bytes(b"session A's content"),
                    "baseline must be the dead session's own H0, not disk's H1"
                );
            }
            _ => panic!("expected Diverged when disk moved on since the dead session's baseline"),
        }
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

        let inherited = find_inheritable_draft(&mut conn, &always_dead, doc_id, &disk_hash)
            .expect("find_inheritable_draft");
        match inherited {
            Inherited::Bridged { draft } => assert_eq!(draft, "UNSAVED shared content"),
            _ => panic!("disk unchanged since the dead session's baseline -> expected Bridged"),
        }
    }
}
