//! The conflict-lifecycle comparison. Pure SQLite: never touches disk (`sync`/
//! `sync_with_theirs` are Update-safe); `probe::probe` is the disk-touching
//! counterpart that records a fresh observation first, making ITS OWN new
//! observation the newest by construction, then calls `sync_with_theirs`
//! with it.

use rusqlite::{Transaction, params};

use crate::Error;
use crate::observation::{self, ObsId};

/// A comparable fact for the Sync/Probe three-way comparison: a content
/// hash, optionally correlated to the [`crate::observation::Observation`]
/// it came from. An out-of-band validity bit is instead modeled as
/// `Option<Version>` at the call site (this crate's own "Options for
/// absent facts" rule).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub hash: String,
    pub obs: Option<ObsId>,
}

/// Discriminates the outcome of comparing buffer/saved/ancestor state for a
/// document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncKind {
    /// The buffer matches what we believe is on disk (or there is no disk
    /// fact yet — an untitled document with an empty buffer).
    Clean,
    /// Only the buffer has changed since the ancestor — an ordinary unsaved
    /// edit; disk has not moved.
    BufferAhead,
    /// Only disk has changed since the ancestor — an external edit landed
    /// while the buffer stayed untouched; safe to adopt.
    DiskAhead,
    /// Both the buffer and disk changed since the ancestor (or there is no
    /// ancestor to reason from at all) — a real conflict.
    Diverged,
}

/// The result of comparing three hashes for a document: the buffer head
/// (`ours`), the freshest disk knowledge (`theirs`), and the derived
/// 3-way-merge ancestor.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncState {
    pub kind: SyncKind,
    pub ancestor: Option<Version>,
    pub ours: Version,
    pub theirs: Option<Version>,
}

/// The SHA-256 of the empty string — the "nothing to save yet" baseline for
/// a document with no disk fact at all.
fn empty_hash() -> &'static str {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<String> = OnceLock::new();
    EMPTY.get_or_init(|| observation::hash_bytes(b""))
}

/// The Conflict lifecycle comparison.
pub fn classify_sync(
    ancestor: Option<&Version>,
    ours: &Version,
    theirs: Option<&Version>,
) -> SyncKind {
    let Some(theirs) = theirs else {
        return if ours.hash == empty_hash() {
            SyncKind::Clean
        } else {
            SyncKind::BufferAhead
        };
    };
    if ours.hash == theirs.hash {
        return SyncKind::Clean;
    }
    let Some(ancestor) = ancestor else {
        return SyncKind::Diverged;
    };
    if theirs.hash == ancestor.hash {
        SyncKind::BufferAhead
    } else if ours.hash == ancestor.hash {
        SyncKind::DiskAhead
    } else {
        SyncKind::Diverged
    }
}

/// Compares the journal reconstruction, the newest recorded observation
/// (ANY origin, ANY session — "theirs"), and the derived ancestor for
/// `doc_id`, AS SEEN BY `session_id`.
pub fn sync(tx: &Transaction<'_>, session_id: i64, doc_id: i64) -> Result<SyncState, Error> {
    let newest = observation::newest_observation(tx, doc_id)?;
    let theirs = newest.map(|o| Version {
        hash: o.blob_hash,
        obs: Some(o.id),
    });
    sync_with_theirs(tx, session_id, doc_id, theirs)
}

/// The ours/ancestor reconstruction shared by [`sync`] (theirs = the newest
/// recorded observation) and `probe::probe` (theirs = a just-recorded fresh
/// observation), including the
/// undo-unwind override: a `DiskAhead` classification
/// upgrades to `Diverged` when this session has recorded ANY correlated
/// observation past `pos` — a resolution the buffer has since been undone
/// past.
pub fn sync_with_theirs(
    tx: &Transaction<'_>,
    session_id: i64,
    doc_id: i64,
    theirs: Option<Version>,
) -> Result<SyncState, Error> {
    let pos = crate::journal::current_seq(tx, session_id, doc_id)?;
    let ours_content = crate::snapshot::recover_document(tx, session_id, doc_id)?;
    let ours = Version {
        hash: observation::hash_bytes(ours_content.as_bytes()),
        obs: None,
    };

    let exclude = theirs.as_ref().and_then(|v| v.obs);
    let ancestor_obs = observation::ancestor_at(tx, doc_id, session_id, pos, exclude)?;
    let ancestor = ancestor_obs.map(|o| Version {
        hash: o.blob_hash,
        obs: Some(o.id),
    });

    let mut kind = classify_sync(ancestor.as_ref(), &ours, theirs.as_ref());

    if kind == SyncKind::DiskAhead {
        let unwound: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM observations WHERE doc_id=?1 AND session_id=?2 AND seq IS NOT NULL AND seq > ?3)",
            params![doc_id, session_id, pos],
            |r| r.get(0),
        )?;
        if unwound {
            kind = SyncKind::Diverged;
        }
    }

    Ok(SyncState {
        kind,
        ancestor,
        ours,
        theirs,
    })
}

/// Dirty ⟺ ours differs from ancestor (`BufferAhead` or `Diverged`) — NEVER
/// `kind != Clean`, which would also flag `DiskAhead` (a pure external
/// change with nothing of the user's unsaved) as phantom-dirty.
pub fn is_dirty(kind: SyncKind) -> bool {
    matches!(kind, SyncKind::BufferAhead | SyncKind::Diverged)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::time::SystemTime;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn seed_doc(tx: &Transaction<'_>) -> i64 {
        tx.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        tx.last_insert_rowid()
    }

    #[test]
    fn untitled_empty_buffer_with_no_disk_fact_is_clean() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::Clean);
        tx.commit().expect("commit");
    }

    #[test]
    fn untitled_nonempty_buffer_with_no_disk_fact_is_buffer_ahead() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "hi".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::BufferAhead);
        tx.commit().expect("commit");
    }

    #[test]
    fn no_ancestor_with_ours_ne_theirs_is_diverged() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let hash = crate::blob::put_blob(&tx, b"some content").expect("seed blob");

        // A bare (uncorrelated) sighting: never ancestor-eligible.
        crate::observation::record_observation(
            &tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &hash,
                seq: None,
                origin: "watch",
            },
            &crate::observation::StatFacts {
                size: 1,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("record");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::Diverged);
        tx.commit().expect("commit");
    }

    /// Undo-unwind override: a resolve observation
    /// correlates `theirs` to the edit seq that resolved it. Undoing the
    /// buffer back BELOW that seq makes `ancestor_at` recompute an OLDER
    /// ancestor the wound-back buffer coincidentally matches — plain
    /// `classify_sync` alone would read that as the safe `DiskAhead`
    /// auto-adopt case. It isn't: the resolution's chain to `theirs` no
    /// longer exists at the wound-back position, so this must re-raise as
    /// `Diverged`.
    #[test]
    fn undo_unwind_upgrades_disk_ahead_to_diverged() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        // An ancestor-eligible 'load' observation at seq 0, matching the
        // (empty) journal reconstruction at that position.
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

        // Journal "hello" at seq 1, then record a 'resolve' observation
        // correlated to seq 1 (matching the reconstruction there) — this is
        // `theirs`, the resolution the undo below will wind back past.
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
        let hello_hash = crate::blob::put_blob(&tx, b"hello").expect("seed hello blob");
        crate::observation::record_observation(
            &tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &hello_hash,
                seq: Some(1),
                origin: "resolve",
            },
            &crate::observation::StatFacts {
                size: 5,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("record resolve observation");

        // Undo back to position 0 — BELOW the resolve observation's seq 1.
        crate::journal::move_undo_pos(&tx, session_id, doc_id, 0).expect("move_undo_pos");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(
            state.kind,
            SyncKind::Diverged,
            "undo past a resolution must re-raise Diverged, never the plain DiskAhead classify_sync alone would compute"
        );
        tx.commit().expect("commit");
    }
}
