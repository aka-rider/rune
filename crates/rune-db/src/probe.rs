//! `Probe` — refreshes a document's disk fact. Ported from Go's probe.
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
//! Go invariant I1).

use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::adopt;
use crate::observation;
use crate::retry;
use crate::sync::{self, SyncKind, SyncState, Version};

/// Refreshes `doc_id`'s disk fact and returns the resulting [`SyncState`].
/// A `documents.path` of `""` (untitled/scratch/chat) has nothing on disk
/// to probe and degrades to a pure [`sync::sync`]. A target that has gone
/// missing surfaces [`Error::NotFound`] — the workspace layer's
/// deleted-guard trigger (WP5). Port of `probe.go:38-102`.
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

    let data = vfs.read(&resolved).map_err(Error::Io)?;
    // Recorded as a raw-bytes blob regardless of UTF-8 validity — a probe is
    // a passive observation of whatever is actually on disk (blob.rs module
    // doc); it must never hard-fail just because the file isn't valid text.
    let hash = retry::with_retry(conn, |tx| crate::blob::put_blob(tx, &data))?;

    let fresh = observation::observe_from_stat(
        conn,
        vfs,
        session_id,
        doc_id,
        &resolved,
        observation::ObservationMeta {
            blob_hash: &hash,
            seq: None,
            origin: "probe",
        },
        now,
    )?;

    let theirs = Some(Version {
        hash: fresh.blob_hash.clone(),
        obs: Some(fresh.id),
    });
    let state = retry::with_retry(conn, |tx| {
        sync::sync_with_theirs(tx, session_id, doc_id, theirs.clone())
    })?;

    if state.kind == SyncKind::Clean {
        // Auto-adopt only when there is something to heal: stacking a fresh
        // 'resolve' adoption on every clean probe tick would grow
        // observations/supersedes unboundedly (probe.go:80-100).
        let should_adopt = retry::with_retry(conn, |tx| {
            let cur = observation::saved_obs_for(tx, session_id, doc_id)?;
            Ok::<bool, Error>(match cur {
                None => true,
                Some(c) => c.blob_hash != fresh.blob_hash,
            })
        })?;
        if should_adopt {
            let pos = retry::with_retry(conn, |tx| {
                crate::journal::current_seq(tx, session_id, doc_id)
            })?;
            let _ = adopt::adopt_equal(conn, session_id, doc_id, fresh.id, pos, now)?;
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
