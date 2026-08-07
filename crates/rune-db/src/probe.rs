//! `Probe` — refreshes a document's disk fact.
//! Unconditionally reads and hashes the live
//! target, records the sighting as `origin='probe'`, and classifies the
//! result — never moving `saved_obs` on a bare divergence (a probe is
//! passive observation, not consent to overwrite). The ONE exception is
//! auto-adopt: when the fresh read hash-equals the journal-head
//! reconstruction (`SyncKind::Clean`), this is unambiguously our own prior
//! write (most likely a `Materialize` whose ack tx never committed before a
//! crash) and is promoted to a real adoption via `adopt::adopt_equal`.
//!
//! Runs as a writer-FIFO op (`OpKind::Probe`): every step below either does
//! `vfs` I/O with no transaction open, or opens its own short
//! `retry::with_retry` transaction — never both at once (plan binding rule,
//! invariant I1).

use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::adopt;
use crate::bracket;
use crate::observation;
use crate::retry;
use crate::sync::{self, SyncKind, SyncState, Version};

/// Refreshes `doc_id`'s disk fact and returns the resulting [`SyncState`].
/// A `documents.path` of `""` (untitled/scratch/chat) has nothing on disk
/// to probe and degrades to a pure [`sync::sync`]. A target that has gone
/// missing surfaces [`Error::NotFound`] — the workspace layer's
/// deleted-guard trigger (WP5).
pub fn probe(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: i64,
    doc_id: i64,
    now: SystemTime,
) -> Result<SyncState, Error> {
    let path: String = retry::with_retry(conn, |tx| {
        tx.query_row(
            "SELECT path FROM documents WHERE id=?1",
            params![doc_id],
            |r| r.get(0),
        )
        .map_err(Error::from)
    })?;

    if path.is_empty() {
        return retry::with_retry(conn, |tx| sync::sync(tx, session_id, doc_id));
    }

    let path = PathBuf::from(path);
    let resolved = vfs.resolve(&path).map_err(Error::Io)?;

    if let Err(e) = vfs.stat(&resolved) {
        if e.kind() == io::ErrorKind::NotFound {
            return Err(Error::NotFound(format!(
                "probe doc {doc_id}: {}",
                path.display()
            )));
        }
        return Err(Error::Io(e));
    }

    // Stat short-circuit (plan Gotchas [R2]): a probe's default action reads
    // the whole file and inserts a fresh observation on EVERY call, which
    // would grow the store unboundedly if enqueued on every tab switch. When
    // the live stat identity/size/mtime already match the newest recorded
    // observation AND that observation is CONFIRMED, nothing on disk has
    // moved since that sighting was taken — classify against it directly,
    // with no re-read and no new row. An unconfirmed newest observation never
    // short-circuits: it decides nothing, including "nothing changed".
    let stat = observation::stat_identity(vfs, &resolved);
    let existing = retry::with_retry(conn, |tx| observation::newest_observation(tx, doc_id))?;
    let unchanged = existing.filter(|o| {
        o.confirmed == Some(true)
            && o.size == stat.size
            && o.mtime == stat.mtime
            && o.inode == stat.inode
            && o.device == stat.device
    });

    let theirs_obs = match unchanged {
        Some(obs) => obs,
        None => {
            // Recorded as a raw-bytes blob regardless of UTF-8 validity — a
            // probe is a passive observation of whatever is actually on disk
            // (blob.rs module doc); it must never hard-fail just because the
            // file isn't valid text. `observe_disk` brackets the read
            // (stat-read-stat) and folds in the suspicious-shrink gate before
            // recording it — a probe can never trust a read caught
            // mid-external-rewrite as a stable fact.
            bracket::observe_disk(
                conn,
                vfs,
                session_id,
                doc_id,
                &resolved,
                bracket::ObserveDiskMeta {
                    seq: None,
                    origin: "probe",
                },
                now,
            )?
        }
    };

    let theirs = Some(Version {
        hash: theirs_obs.blob_hash.clone(),
        obs: Some(theirs_obs.id),
    });
    let state = retry::with_retry(conn, |tx| {
        sync::sync_with_theirs(tx, session_id, doc_id, theirs.clone())
    })?;

    if state.kind == SyncKind::Clean {
        // Auto-adopt only when there is something to heal: stacking a fresh
        // 'resolve' adoption on every clean probe tick would grow
        // observations/supersedes unboundedly.
        let should_adopt = retry::with_retry(conn, |tx| {
            let cur = observation::saved_obs_for(tx, session_id, doc_id)?;
            Ok::<bool, Error>(match cur {
                None => true,
                Some(c) => c.blob_hash != theirs_obs.blob_hash,
            })
        })?;
        if should_adopt {
            let pos = retry::with_retry(conn, |tx| {
                crate::journal::current_seq(tx, session_id, doc_id)
            })?;
            let _ = adopt::adopt_equal(conn, session_id, doc_id, theirs_obs.id, pos, now)?;
        }
    }

    Ok(state)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_vfs::Mem;
    use std::path::Path;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
        let temp = vfs.write_durable(path, bytes).expect("write_durable");
        vfs.rename_excl(&temp, path).expect("publish");
    }

    #[test]
    fn probe_on_untitled_document_is_a_pure_sync() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();

        let state = probe(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("probe");
        assert_eq!(state.kind, SyncKind::Clean);
    }

    #[test]
    fn probe_on_a_deleted_target_surfaces_not_found() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/gone.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();

        let err =
            probe(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect_err("must error");
        assert!(matches!(err, Error::NotFound(_)));
    }

    /// Plan Gotchas `[R2]`/WP2.S4: a second probe against a file whose stat
    /// identity/size/mtime haven't moved since the first probe's own
    /// observation must classify against that stored fact directly — no
    /// re-read, no second `observations` row.
    #[test]
    fn probe_stat_short_circuit_skips_reading_and_inserting_when_unchanged() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"hello");

        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();

        let count = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM observations WHERE doc_id=?1",
                params![doc_id],
                |r| r.get(0),
            )
            .expect("count observations")
        };

        let first = probe(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("probe");
        assert_eq!(
            first.kind,
            SyncKind::Diverged,
            "no ancestor and ours (empty) != theirs (hello): a real divergence"
        );
        assert_eq!(
            count(&conn),
            1,
            "the first probe records exactly one observation"
        );

        let second =
            probe(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("probe again");
        assert_eq!(
            second.kind, first.kind,
            "an unchanged file must classify identically the second time"
        );
        assert_eq!(
            count(&conn),
            1,
            "an unchanged stat must short-circuit: no second observation inserted"
        );
    }

    /// The full completed-merge sequence: load, buffer edit, external disk
    /// rewrite, a probe that sees the divergence, then `resolve_adopt`
    /// against that disk observation at the current journal head (the
    /// zero-conflict / Discard shape — no install edit follows). The next
    /// probe finds the disk untouched, takes the stat short-circuit, and
    /// reuses the resolve row as theirs: it must read the reconciliation
    /// for what it is, never re-fabricate Diverged.
    #[test]
    fn probe_after_resolve_adopt_with_unchanged_disk_is_not_diverged() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"one");

        let loaded = crate::load::load(
            &mut conn,
            &vfs,
            session_id,
            &|_, _| false,
            path,
            SystemTime::now(),
        )
        .expect("load");
        let doc_id = loaded.doc_id;

        {
            let tx = conn.transaction().expect("tx");
            crate::journal::append_edit(
                &tx,
                session_id,
                SystemTime::now(),
                doc_id,
                &[rune_core::buffer::AppliedEdit {
                    start: 0,
                    end: 3,
                    deleted: "one".to_string(),
                    insert: "merged".to_string(),
                }],
                &[],
                &[],
            )
            .expect("append_edit");
            tx.commit().expect("commit");
        }

        let temp = vfs.write_durable(path, b"two two").expect("write_durable");
        vfs.exchange(&temp, path).expect("exchange");
        let diverged =
            probe(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("probe");
        assert_eq!(diverged.kind, SyncKind::Diverged, "external rewrite lands");
        let disk_obs = diverged
            .theirs
            .as_ref()
            .and_then(|t| t.obs)
            .expect("disk observation");

        let head = retry::with_retry(&mut conn, |tx| {
            crate::journal::current_seq(tx, session_id, doc_id)
        })
        .expect("head seq");
        adopt::resolve_adopt(
            &mut conn,
            session_id,
            doc_id,
            disk_obs,
            Some(head),
            SystemTime::now(),
        )
        .expect("resolve_adopt");

        let after =
            probe(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("probe again");
        assert_eq!(
            after.kind,
            SyncKind::BufferAhead,
            "a reconciled buffer with untouched disk is an ordinary unsaved edit, never Diverged"
        );
    }

    /// Task WP-A(2i): a newest observation that is UNCONFIRMED (an empty
    /// read caught mid-external-rewrite, say) must never satisfy the stat
    /// short-circuit even when its stat facts happen to match the live
    /// stat exactly — an unconfirmed fact decides nothing, including
    /// "nothing changed". The probe must re-read for real.
    #[test]
    fn probe_stat_short_circuit_never_fires_on_an_unconfirmed_observation() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"hello");

        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();

        // Seed an UNCONFIRMED observation whose stat facts already match
        // the live file exactly — the only way the short-circuit could ever
        // fire on it.
        let stat = observation::stat_identity(&vfs, path);
        {
            let tx = conn.transaction().expect("tx");
            let hash = crate::blob::put_blob(&tx, b"hello").expect("seed blob");
            observation::record_observation(
                &tx,
                doc_id,
                session_id,
                observation::ObservationMeta {
                    blob_hash: &hash,
                    seq: None,
                    origin: "probe",
                    confirmed: Some(false),
                },
                &stat,
                "t",
            )
            .expect("seed unconfirmed observation");
            tx.commit().expect("commit");
        }

        let count = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM observations WHERE doc_id=?1",
                params![doc_id],
                |r| r.get(0),
            )
            .expect("count observations")
        };
        assert_eq!(count(&conn), 1, "test setup: one unconfirmed observation");

        probe(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("probe");

        assert_eq!(
            count(&conn),
            2,
            "an unconfirmed newest observation must never short-circuit a real re-read"
        );
    }

    #[test]
    fn probe_auto_adopts_when_disk_hash_equals_journal_head() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"hello");

        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();

        // Journal reconstruction at head must equal disk content ("hello")
        // for auto-adopt to trigger — journal an edit that produces it.
        {
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
                    insert: "hello".to_string(),
                }],
                &[],
                &[],
            )
            .expect("append_edit");
            tx.commit().expect("commit");
        }

        let state = probe(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("probe");
        assert_eq!(state.kind, SyncKind::Clean);

        let saved_obs: Option<i64> = conn
            .query_row(
                "SELECT saved_obs FROM session_documents WHERE session_id=?1 AND doc_id=?2",
                params![session_id, doc_id],
                |r| r.get(0),
            )
            .expect("read saved_obs");
        assert!(
            saved_obs.is_some(),
            "clean probe with no prior baseline must auto-adopt"
        );

        let origin: String = conn
            .query_row(
                "SELECT origin FROM observations WHERE id=?1",
                params![saved_obs.unwrap()],
                |r| r.get(0),
            )
            .expect("read origin");
        assert_eq!(
            origin, "resolve",
            "auto-adopt must be a real, ancestor-eligible adoption"
        );
    }
}
