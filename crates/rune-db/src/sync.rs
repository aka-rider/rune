//! The conflict-lifecycle comparison. Pure SQLite: never touches disk (`sync`/
//! `sync_with_theirs` are Update-safe); `probe::probe` is the disk-touching
//! counterpart that records a fresh observation first, making ITS OWN new
//! observation the newest by construction, then calls `sync_with_theirs`
//! with it.

use rusqlite::{OptionalExtension, Transaction, params};

use crate::Error;
#[cfg(test)]
use crate::ids::Seq;
use crate::ids::{BlobHash, DocId, ObsId, SessionId};
use crate::obs_origin::ObsOrigin;
use crate::observation;

#[cfg(test)]
use crate::confirmation::Confirmation;

/// A comparable fact for the Sync/Probe three-way comparison: a content
/// hash, optionally correlated to the [`crate::observation::Observation`]
/// it came from. An out-of-band validity bit is instead modeled as
/// `Option<Version>` at the call site (this crate's own "Options for
/// absent facts" rule).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub hash: BlobHash,
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
fn empty_hash() -> &'static BlobHash {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<BlobHash> = OnceLock::new();
    EMPTY.get_or_init(|| BlobHash(observation::hash_bytes(b"")))
}

/// The Conflict lifecycle comparison.
pub fn classify_sync(
    ancestor: Option<&Version>,
    ours: &Version,
    theirs: Option<&Version>,
) -> SyncKind {
    let Some(theirs) = theirs else {
        return if &ours.hash == empty_hash() {
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
/// hash coincidence must not make it invisible.
fn theirs_is_this_sessions_newest_publish(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    theirs: Option<&Version>,
) -> Result<bool, Error> {
    let Some(theirs) = theirs else {
        return Ok(false);
    };
    let newest_publish: Option<(String, SessionId)> = tx
        .query_row(
            "SELECT blob_hash, session_id FROM observations WHERE doc_id=?1 AND origin=?2 ORDER BY id DESC LIMIT 1",
            params![doc_id, ObsOrigin::Save],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(matches!(
        newest_publish,
        Some((hash, publisher)) if hash == theirs.hash.as_str() && publisher == session_id
    ))
}

fn buffer_unwound_past(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
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
pub fn sync(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
) -> Result<SyncState, Error> {
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
    session_id: SessionId,
    doc_id: DocId,
    theirs: Option<Version>,
) -> Result<SyncState, Error> {
    let pos = crate::journal::current_seq(tx, session_id, doc_id)?;
    let ours_content = crate::snapshot::recover_document(tx, session_id, doc_id)?.content;
    let ours = Version {
        hash: BlobHash(observation::hash_bytes(ours_content.as_bytes())),
        obs: None,
    };

    let exclude = theirs.as_ref().and_then(|v| v.obs);
    let ancestor_obs = observation::ancestor_at(tx, doc_id, session_id, pos.0, exclude)?;
    let ancestor = ancestor_obs.map(|o| Version {
        hash: o.blob_hash,
        obs: Some(o.id),
    });

    let mut kind = classify_sync(ancestor.as_ref(), &ours, theirs.as_ref());

    if kind == SyncKind::DiskAhead && buffer_unwound_past(tx, session_id, doc_id, pos.0)? {
        kind = SyncKind::Diverged;
    }

    if kind.is_disk_divergent()
        && theirs_is_this_sessions_newest_publish(tx, session_id, doc_id, theirs.as_ref())?
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

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sync_merge_resolve_tests.rs"]
mod merge_resolve_tests;
