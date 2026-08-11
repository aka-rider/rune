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

impl SyncKind {
    /// The disk holds changes the buffer doesn't (`DiskAhead`/`Diverged`) —
    /// the one predicate behind every "disk changed" affordance: the merge
    /// invitation, the footer/tab divergence markers, and merge entry's own
    /// pre-check. `BufferAhead` is deliberately excluded: an ordinary unsaved
    /// edit is the dirty flag's job, not a divergence.
    pub fn is_disk_divergent(self) -> bool {
        matches!(self, SyncKind::DiskAhead | SyncKind::Diverged)
    }
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

/// A user cannot conflict with their own changes, but a change nobody
/// showed them is still a change: an external revert to bytes rune once
/// published hides itself exactly like any other external rewrite, and the
/// hash coincidence must not make it invisible. Both halves are therefore
/// required — rune PUBLISHED these bytes (`origin='save'`, any session,
/// `theirs`' own row included), AND the disk state DESCENDS from what this
/// session last agreed on.
fn theirs_is_our_own_published_descendant(
    tx: &Transaction<'_>,
    doc_id: i64,
    ancestor: Option<&Version>,
    theirs: Option<&Version>,
) -> Result<bool, Error> {
    let (Some(ancestor), Some(theirs)) = (ancestor, theirs) else {
        return Ok(false);
    };
    let (Some(ancestor_id), Some(theirs_id)) = (ancestor.obs, theirs.obs) else {
        return Ok(false);
    };
    let published: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM observations WHERE doc_id=?1 AND blob_hash=?2 AND origin='save')",
        params![doc_id, theirs.hash],
        |r| r.get(0),
    )?;
    Ok(published && crate::lineage::is_ancestor(tx, ancestor_id, theirs_id)?)
}

fn buffer_unwound_past(
    tx: &Transaction<'_>,
    session_id: i64,
    doc_id: i64,
    pos: i64,
) -> Result<bool, Error> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM observations WHERE doc_id=?1 AND session_id=?2 AND seq IS NOT NULL AND seq > ?3)",
        params![doc_id, session_id, pos],
        |r| r.get(0),
    )?)
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
/// observation), including the undo-unwind override: an unwound buffer is
/// never plain `DiskAhead`, since adopting would silently drop the undo.
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

    if kind == SyncKind::DiskAhead && buffer_unwound_past(tx, session_id, doc_id, pos)? {
        kind = SyncKind::Diverged;
    }

    if kind.is_disk_divergent()
        && theirs_is_our_own_published_descendant(tx, doc_id, ancestor.as_ref(), theirs.as_ref())?
    {
        kind = SyncKind::BufferAhead;
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

    fn stat_at(mtime: &str) -> crate::observation::StatFacts {
        crate::observation::StatFacts {
            size: Some(1),
            mtime: Some(mtime.to_string()),
            ..Default::default()
        }
    }

    /// How the disk bytes this session is about to classify came to be
    /// there: who (if anyone) published them, and whether the observation
    /// carrying them descends from what this session last agreed on — the
    /// edge `adopt::record_adoption_tx` and a confirmed fresh sighting both
    /// record in the real store.
    struct DiskSighting<'a> {
        blob: &'a [u8],
        published_by: Option<i64>,
        descends_from_our_ancestor: bool,
    }

    /// Seeds a document this session loaded and then edited past, and hands
    /// back the classification against `sighting` freshly seen on disk.
    fn classify_fresh_disk_sighting(
        tx: &Transaction<'_>,
        session_id: i64,
        doc_id: i64,
        sighting: DiskSighting<'_>,
    ) -> SyncKind {
        let disk_hash = crate::blob::put_blob(tx, sighting.blob).expect("seed disk blob");
        if let Some(publisher) = sighting.published_by {
            crate::observation::record_observation(
                tx,
                doc_id,
                publisher,
                crate::observation::ObservationMeta {
                    blob_hash: &disk_hash,
                    seq: Some(0),
                    origin: "save",
                    confirmed: Some(true),
                },
                &stat_at("t0"),
                "t0",
            )
            .expect("record the publisher's save");
        }

        let load_hash = crate::blob::put_blob(tx, b"what this session loaded").expect("seed blob");
        let load_id = crate::observation::record_observation(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &load_hash,
                seq: Some(0),
                origin: "load",
                confirmed: None,
            },
            &stat_at("t1"),
            "t1",
        )
        .expect("record this session's load");

        crate::journal::append_edit(
            tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "this session's own unsaved edit".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit");

        let theirs_id = crate::observation::insert_observation_row(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &disk_hash,
                seq: None,
                origin: "probe",
                confirmed: Some(true),
            },
            &stat_at("t2"),
            "t2",
            crate::observation::ParentEdges {
                a: sighting.descends_from_our_ancestor.then_some(load_id),
                b: None,
            },
        )
        .expect("record the fresh disk sighting");

        let theirs = Version {
            hash: disk_hash,
            obs: Some(theirs_id),
        };
        sync_with_theirs(tx, session_id, doc_id, Some(theirs))
            .expect("sync_with_theirs")
            .kind
    }

    /// A user cannot conflict with their own changes: bytes ANOTHER rune
    /// session published are still rune's own hand, so rediscovering them on
    /// disk while this session has unsaved edits is an ordinary unsaved
    /// edit — never an invitation to merge against our own content.
    #[test]
    fn a_fresh_sighting_of_another_sessions_save_is_buffer_ahead() {
        let mut conn = open();
        let publisher =
            crate::session::establish_session(&conn, SystemTime::now()).expect("publisher session");
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("this session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let kind = classify_fresh_disk_sighting(
            &tx,
            session_id,
            doc_id,
            DiskSighting {
                blob: b"bytes rune published",
                published_by: Some(publisher),
                descends_from_our_ancestor: true,
            },
        );
        assert_eq!(
            kind,
            SyncKind::BufferAhead,
            "bytes any rune session published, on a disk state we still descend from, are ours to overwrite"
        );
        tx.commit().expect("commit");
    }

    /// The authorship half on its own: bytes no `save` ever published belong
    /// to whoever put them there. A confirmed read of them is knowledge,
    /// never authorization, however cleanly they chain off our ancestor.
    #[test]
    fn a_fresh_sighting_rune_never_published_stays_diverged() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let kind = classify_fresh_disk_sighting(
            &tx,
            session_id,
            doc_id,
            DiskSighting {
                blob: b"a stranger's rewrite",
                published_by: None,
                descends_from_our_ancestor: true,
            },
        );
        assert_eq!(
            kind,
            SyncKind::Diverged,
            "a stranger's bytes stay a conflict however confidently they were read"
        );
        tx.commit().expect("commit");
    }

    /// The containment half on its own, and the case that decides the rule:
    /// something outside rune — a `git checkout`, a restored backup — put
    /// bytes on disk that rune itself published at some earlier point. The
    /// hash says "ours"; the missing lineage edge says the current disk
    /// state does not descend from what this session last agreed on. The
    /// external change was hidden from the user either way, and a hash
    /// coincidence must not make it invisible.
    #[test]
    fn an_external_revert_to_bytes_rune_published_stays_a_conflict() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let kind = classify_fresh_disk_sighting(
            &tx,
            session_id,
            doc_id,
            DiskSighting {
                blob: b"bytes rune published long ago",
                published_by: Some(session_id),
                descends_from_our_ancestor: false,
            },
        );
        assert_eq!(
            kind,
            SyncKind::Diverged,
            "an external revert is an external change; matching a hash we once wrote does not authorize overwriting it"
        );
        tx.commit().expect("commit");
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
                confirmed: None,
            },
            &crate::observation::StatFacts {
                size: Some(1),
                mtime: Some("t".to_string()),
                ..Default::default()
            },
            "t",
        )
        .expect("record");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::Diverged);
        tx.commit().expect("commit");
    }

    /// Seeds the completed-reconciliation shape: a 'load' observation at
    /// seq 0 with `load_blob`, one journaled edit producing "merged" at
    /// seq 1, and a 'resolve' observation correlated to that head seq
    /// carrying `resolve_blob` — the record a zero-conflict merge or
    /// Discard leaves behind, where no further edit moves the journal
    /// past the resolve seq.
    fn seed_resolve_at_head(
        tx: &Transaction<'_>,
        session_id: i64,
        doc_id: i64,
        load_blob: &[u8],
        resolve_blob: &[u8],
    ) {
        let load_hash = crate::blob::put_blob(tx, load_blob).expect("seed load blob");
        crate::observation::record_observation(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &load_hash,
                seq: Some(0),
                origin: "load",
                confirmed: None,
            },
            &crate::observation::StatFacts {
                size: Some(load_blob.len() as i64),
                mtime: Some("t".to_string()),
                ..Default::default()
            },
            "t",
        )
        .expect("record load observation");

        crate::journal::append_edit(
            tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "merged".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit");

        let resolve_hash = crate::blob::put_blob(tx, resolve_blob).expect("seed resolve blob");
        crate::observation::record_observation(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &resolve_hash,
                seq: Some(1),
                origin: "resolve",
                confirmed: None,
            },
            &crate::observation::StatFacts {
                size: Some(resolve_blob.len() as i64),
                mtime: Some("t2".to_string()),
                ..Default::default()
            },
            "t2",
        )
        .expect("record resolve observation");
    }

    /// The zero-conflict-merge / all-both shape: the resolve observation
    /// sits at exactly the journal head, and its blob (the disk bytes) is
    /// NOT what the buffer reconstructs to. The resolve row is both theirs
    /// and the legitimate ancestor — only the buffer moved since the
    /// reconciliation, so this is an ordinary unsaved edit, never a
    /// fabricated conflict.
    #[test]
    fn resolve_at_head_seq_classifies_buffer_ahead_not_diverged() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        seed_resolve_at_head(&tx, session_id, doc_id, b"original", b"disk");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(
            state.kind,
            SyncKind::BufferAhead,
            "a resolve observation at the head seq is a completed reconciliation, not a divergence"
        );
        tx.commit().expect("commit");
    }

    /// The Discard shape: the resolve observation's blob equals the journal
    /// reconstruction — buffer and disk agree byte-for-byte.
    #[test]
    fn resolve_at_head_seq_matching_reconstruction_is_clean() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        seed_resolve_at_head(&tx, session_id, doc_id, b"original", b"merged");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::Clean);
        tx.commit().expect("commit");
    }

    /// Pins the already-working case: once an edit moves the journal PAST
    /// the resolve seq, the resolve row is an ordinary older correlation
    /// and classification stays BufferAhead.
    #[test]
    fn edit_after_resolve_still_classifies_buffer_ahead() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        seed_resolve_at_head(&tx, session_id, doc_id, b"original", b"disk");

        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: 6,
                end: 6,
                deleted: String::new(),
                insert: " more".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit after resolve");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::BufferAhead);
        tx.commit().expect("commit");
    }

    /// Seeds the undo-unwind shape: a 'load' observation at seq 0 carrying
    /// the empty journal reconstruction, one journaled edit producing
    /// "hello" at seq 1, an observation of "hello" with `origin` correlated
    /// to seq 1 and chained off the load the way `adopt::record_adoption_tx`
    /// chains every new row off the `saved_obs` it replaces, and the buffer
    /// then undone back to seq 0.
    fn seed_unwind_past_disk_bytes(
        tx: &Transaction<'_>,
        session_id: i64,
        doc_id: i64,
        origin: &str,
    ) {
        let empty_hash = crate::blob::put_blob(tx, b"").expect("seed empty blob");
        let load_id = crate::observation::record_observation(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &empty_hash,
                seq: Some(0),
                origin: "load",
                confirmed: None,
            },
            &crate::observation::StatFacts {
                mtime: Some("t".to_string()),
                ..Default::default()
            },
            "t",
        )
        .expect("record load observation");

        crate::journal::append_edit(
            tx,
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

        let hello_hash = crate::blob::put_blob(tx, b"hello").expect("seed hello blob");
        crate::observation::insert_observation_row(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &hello_hash,
                seq: Some(1),
                origin,
                confirmed: None,
            },
            &crate::observation::StatFacts {
                size: Some(5),
                mtime: Some("t2".to_string()),
                ..Default::default()
            },
            "t2",
            crate::observation::ParentEdges {
                a: Some(load_id),
                b: None,
            },
        )
        .expect("record disk observation");

        crate::journal::move_undo_pos(tx, session_id, doc_id, 0).expect("move_undo_pos");
    }

    /// Undoing past bytes rune itself published leaves the user's own save on
    /// disk and only the buffer moved — an ordinary unsaved edit, never a
    /// conflict against the user's own content.
    #[test]
    fn unwind_past_our_own_published_save_is_buffer_ahead_not_a_conflict() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        seed_unwind_past_disk_bytes(&tx, session_id, doc_id, "save");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(
            state.kind,
            SyncKind::BufferAhead,
            "disk holds bytes rune published; undoing past them must not offer a merge against our own content"
        );
        tx.commit().expect("commit");
    }

    /// A resolve records bytes accepted INTO the buffer, never bytes written
    /// out. Undoing past it withdraws exactly that acceptance, so nothing
    /// authorizes overwriting whoever put those bytes on disk.
    #[test]
    fn unwind_past_an_unpublished_resolve_adoption_stays_diverged() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        seed_unwind_past_disk_bytes(&tx, session_id, doc_id, "resolve");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(
            state.kind,
            SyncKind::Diverged,
            "undo past an adoption nobody published must stay a conflict, never the plain DiskAhead classify_sync alone would compute"
        );
        tx.commit().expect("commit");
    }
}
