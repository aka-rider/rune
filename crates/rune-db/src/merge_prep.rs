//! `MergePrep` — plan WP3.S1's merge-entry fresh-state read: runs
//! [`probe::probe`] (the same disk fact refresh a tab-switch probe does)
//! and returns the ancestor/theirs BYTES from the same op, not just their
//! hashes. `sync.rs`'s own `SyncState` carries hashes only (plan Gotchas
//! `[B2]`: "No existing non-blocking path returns blob bytes to the TUI")
//! — merge entry needs the actual content to feed `rune_merge::merge_hunks`,
//! and re-deriving it from a LATER, separately-timed read would reopen
//! exactly the race `[B2]` exists to close. One writer-thread op, one
//! decisive moment: nothing else can move `saved_obs`/the newest
//! observation between the probe and the blob reads below, since this
//! whole op runs to completion before the writer thread looks at its next
//! queued op.

use std::time::SystemTime;

use rusqlite::Connection;

use rune_vfs::Vfs;

use crate::Error;
use crate::blob;
use crate::observation::ObsId;
use crate::probe;
use crate::sync::SyncState;

/// `MergePrep`'s result: the freshly classified [`SyncState`] plus the
/// actual ancestor/theirs bytes it was classified from. `theirs`/
/// `theirs_obs` are meaningful only when `sync.kind` is `DiskAhead`/
/// `Diverged` (the only kinds `classify_sync` can ever produce with a
/// `theirs` version present) — the caller's authoritative gate (plan
/// WP3.S6) checks `sync.kind` before ever reading them.
#[derive(Clone, Debug, PartialEq)]
pub struct MergePrepResult {
    pub sync: SyncState,
    pub ancestor: Option<Vec<u8>>,
    pub theirs: Vec<u8>,
    pub theirs_obs: ObsId,
}

/// Runs the fresh-state read for `doc_id`. Port of no single Go function —
/// `workspace_merge_fresh.go`'s entry point re-runs `Probe` then re-reads
/// blobs across two separate calls; this crate collapses that into one op
/// so the TUI's `update` never has to correlate two async round trips for
/// one merge attempt.
pub fn merge_prep(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: i64,
    doc_id: i64,
    now: SystemTime,
) -> Result<MergePrepResult, Error> {
    let sync = probe::probe(conn, vfs, session_id, doc_id, now)?;

    // `theirs`/`theirs_obs` stay at their empty/zero defaults whenever
    // `sync.theirs` is `None` — unreachable in practice with `kind` in
    // `DiskAhead`/`Diverged` (`classify_sync`'s `None` branch only ever
    // yields `Clean`/`BufferAhead`), which is exactly what the caller's
    // authoritative gate checks before trusting either field.
    let theirs_obs = sync.theirs.as_ref().and_then(|v| v.obs).unwrap_or(0);
    let theirs = match &sync.theirs {
        Some(version) => blob::get_blob(conn, &version.hash)?,
        None => Vec::new(),
    };
    let ancestor = match &sync.ancestor {
        Some(version) => Some(blob::get_blob(conn, &version.hash)?),
        None => None,
    };

    Ok(MergePrepResult {
        sync,
        ancestor,
        theirs,
        theirs_obs,
    })
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

    /// Plan WP3 "Done when" (a): a diverged fixture's `MergePrep` reports
    /// `Diverged` and hands back both sides' actual bytes, not just hashes.
    #[test]
    fn merge_prep_on_a_diverged_document_returns_both_sides_bytes() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"theirs content");

        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();

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
                    insert: "ours content".to_string(),
                }],
                &[],
                &[],
            )
            .expect("append_edit");
            tx.commit().expect("commit");
        }

        let result =
            merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");
        assert_eq!(result.sync.kind, crate::sync::SyncKind::Diverged);
        assert_eq!(result.theirs, b"theirs content");
        assert!(result.theirs_obs > 0);
        assert_eq!(result.ancestor, None, "no prior ancestor-eligible sighting");
    }

    /// Plan WP3 "Done when" (b): a `DiskAhead` document (clean buffer, disk
    /// moved) returns the disk bytes as `theirs` with no ancestor divergence
    /// story needed for the fast path.
    #[test]
    fn merge_prep_on_a_disk_ahead_document_returns_theirs_bytes() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"");

        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();

        // A 'load' observation at seq 0 (ancestor-eligible), matching the
        // empty journal reconstruction — the buffer never changed, so any
        // later disk-only change is `DiskAhead`.
        {
            let tx = conn.transaction().expect("tx");
            let empty_hash = crate::blob::put_blob(&tx, b"").expect("seed empty blob");
            crate::observation::record_observation(
                &tx,
                doc_id,
                session_id,
                crate::observation::ObservationMeta {
                    blob_hash: &empty_hash,
                    seq: Some(0),
                    origin: "load",
                },
                &crate::observation::StatFacts {
                    mtime: "t".to_string(),
                    ..Default::default()
                },
                "t",
            )
            .expect("record load observation");
            tx.commit().expect("commit");
        }

        vfs.save_atomic(path, b"disk moved on").expect("overwrite");

        let result =
            merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");
        assert_eq!(result.sync.kind, crate::sync::SyncKind::DiskAhead);
        assert_eq!(result.theirs, b"disk moved on");
    }
}
