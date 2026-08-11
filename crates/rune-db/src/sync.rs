//! The conflict-lifecycle comparison. Pure SQLite: never touches disk (`sync`/
//! `sync_with_theirs` are Update-safe); `probe::probe` is the disk-touching
//! counterpart that records a fresh observation first, making ITS OWN new
//! observation the newest by construction, then calls `sync_with_theirs`
//! with it.

use rusqlite::{OptionalExtension, Transaction, params};

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
/// hash coincidence must not make it invisible.
fn theirs_is_our_newest_publish(
    tx: &Transaction<'_>,
    doc_id: i64,
    theirs: Option<&Version>,
) -> Result<bool, Error> {
    let Some(theirs) = theirs else {
        return Ok(false);
    };
    let newest_publish: Option<String> = tx
        .query_row(
            "SELECT blob_hash FROM observations WHERE doc_id=?1 AND origin='save' ORDER BY id DESC LIMIT 1",
            params![doc_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(newest_publish.as_deref() == Some(theirs.hash.as_str()))
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

    if kind.is_disk_divergent() && theirs_is_our_newest_publish(tx, doc_id, theirs.as_ref())? {
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

    fn stat_of(content: &str, at: &str) -> crate::observation::StatFacts {
        crate::observation::StatFacts {
            size: Some(content.len() as i64),
            mtime: Some(at.to_string()),
            ..Default::default()
        }
    }

    fn open_document(tx: &Transaction<'_>, session_id: i64, doc_id: i64, disk: &str) {
        let now = SystemTime::now();
        let seq = crate::journal::current_seq(tx, session_id, doc_id).expect("current_seq");
        crate::snapshot::create_snapshot(tx, session_id, now, doc_id, disk, seq)
            .expect("anchor the load snapshot");
        let hash = crate::blob::put_blob(tx, disk.as_bytes()).expect("put the load blob");
        crate::adopt::record_adoption_tx(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &hash,
                seq: Some(seq),
                origin: "load",
                confirmed: Some(true),
            },
            &stat_of(disk, "load"),
            "load",
            None,
        )
        .expect("record the load adoption");
    }

    fn type_text(tx: &Transaction<'_>, session_id: i64, doc_id: i64, text: &str) -> i64 {
        let ours = crate::snapshot::recover_document(tx, session_id, doc_id).expect("recover");
        let end = ours.len();
        crate::journal::append_edit(
            tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: end,
                end,
                deleted: String::new(),
                insert: text.to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit")
    }

    fn undo_to(tx: &Transaction<'_>, session_id: i64, doc_id: i64, seq: i64) {
        crate::journal::move_undo_pos(tx, session_id, doc_id, seq).expect("move_undo_pos");
    }

    fn publish_save(tx: &Transaction<'_>, session_id: i64, doc_id: i64, bytes: &str, at: &str) {
        let hash = crate::blob::put_blob(tx, bytes.as_bytes()).expect("put the published blob");
        let seq = crate::journal::current_seq(tx, session_id, doc_id).expect("current_seq");
        crate::adopt::record_adoption_tx(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &hash,
                seq: Some(seq),
                origin: "save",
                confirmed: Some(true),
            },
            &stat_of(bytes, at),
            at,
            None,
        )
        .expect("record the save adoption");
    }

    fn external_write(
        tx: &Transaction<'_>,
        session_id: i64,
        doc_id: i64,
        bytes: &str,
        at: &str,
    ) -> crate::observation::Observation {
        crate::observation::observe_from_stat_tx(
            tx,
            session_id,
            doc_id,
            &stat_of(bytes, at),
            at,
            crate::observation::ObserveInput {
                data: bytes.as_bytes(),
                seq: None,
                origin: "probe",
                confirmed: Some(true),
            },
        )
        .expect("record the fresh disk sighting")
    }

    fn resolve_against(
        tx: &Transaction<'_>,
        session_id: i64,
        doc_id: i64,
        theirs: &crate::observation::Observation,
        edit_seq: i64,
    ) {
        crate::adopt::record_adoption_tx(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &theirs.blob_hash,
                seq: Some(edit_seq),
                origin: "resolve",
                confirmed: theirs.confirmed,
            },
            &theirs.stat(),
            "resolve",
            Some(theirs.id),
        )
        .expect("record the resolve adoption");
    }

    fn verdict(tx: &Transaction<'_>, session_id: i64, doc_id: i64) -> SyncKind {
        sync(tx, session_id, doc_id).expect("sync").kind
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

        open_document(&tx, session_id, doc_id, "base");
        type_text(&tx, session_id, doc_id, "-mine");
        publish_save(
            &tx,
            publisher,
            doc_id,
            "base-theirs",
            "the other session's save",
        );
        external_write(&tx, session_id, doc_id, "base-theirs", "rediscovery");

        assert_eq!(
            verdict(&tx, session_id, doc_id),
            SyncKind::BufferAhead,
            "bytes any rune session published are ours to overwrite"
        );
        tx.commit().expect("commit");
    }

    /// Bytes no `save` ever published belong to whoever put them there. A
    /// confirmed read of them is knowledge, never authorization.
    #[test]
    fn a_fresh_sighting_rune_never_published_stays_diverged() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        open_document(&tx, session_id, doc_id, "base");
        type_text(&tx, session_id, doc_id, "-mine");
        external_write(
            &tx,
            session_id,
            doc_id,
            "a stranger's rewrite",
            "the rewrite",
        );

        assert_eq!(
            verdict(&tx, session_id, doc_id),
            SyncKind::Diverged,
            "a stranger's bytes stay a conflict however confidently they were read"
        );
        tx.commit().expect("commit");
    }

    /// The case that decides the rule: something outside rune — a `git
    /// checkout`, a restored backup — puts bytes on disk that rune itself
    /// published EARLIER, while rune's latest publish said something else.
    /// The hash says "ours"; the newest publish says the file moved behind
    /// the user's back.
    #[test]
    fn an_external_revert_to_bytes_rune_published_stays_a_conflict() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        open_document(&tx, session_id, doc_id, "base");
        type_text(&tx, session_id, doc_id, "-one");
        publish_save(&tx, session_id, doc_id, "base-one", "the first save");
        type_text(&tx, session_id, doc_id, "-two");
        publish_save(&tx, session_id, doc_id, "base-one-two", "the second save");
        external_write(&tx, session_id, doc_id, "base-one", "the revert");

        let kind = verdict(&tx, session_id, doc_id);
        assert!(
            kind.is_disk_divergent(),
            "an external revert is an external change; matching a hash we once wrote does not authorize overwriting it, got {kind:?}"
        );
        tx.commit().expect("commit");
    }

    /// The same revert while the buffer carries unsaved edits — the shape
    /// where the classification the save gate reads is the ONLY thing left
    /// to notice that somebody else touched the file.
    #[test]
    fn an_external_revert_under_unsaved_edits_stays_a_conflict() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        open_document(&tx, session_id, doc_id, "base");
        type_text(&tx, session_id, doc_id, "-one");
        publish_save(&tx, session_id, doc_id, "base-one", "the first save");
        type_text(&tx, session_id, doc_id, "-two");
        publish_save(&tx, session_id, doc_id, "base-one-two", "the second save");
        type_text(&tx, session_id, doc_id, "-unsaved");
        external_write(&tx, session_id, doc_id, "base-one", "the revert");

        let kind = verdict(&tx, session_id, doc_id);
        assert!(
            kind.is_disk_divergent(),
            "a dirty buffer must not hide a revert either, got {kind:?}"
        );
        tx.commit().expect("commit");
    }

    /// Seeds the merge story up to the resolution: this session saves, an
    /// external write lands, the user merges and installs the result. Hands
    /// back the seq the install can be undone past.
    fn merge_after_an_external_write(tx: &Transaction<'_>, session_id: i64, doc_id: i64) -> i64 {
        open_document(tx, session_id, doc_id, "base");
        let saved_seq = type_text(tx, session_id, doc_id, "-one");
        publish_save(
            tx,
            session_id,
            doc_id,
            "base-one",
            "the save before the merge",
        );
        let theirs = external_write(
            tx,
            session_id,
            doc_id,
            "base-external",
            "the external write",
        );
        let install_seq = type_text(tx, session_id, doc_id, "-merged");
        resolve_against(tx, session_id, doc_id, &theirs, install_seq);
        saved_seq
    }

    /// Publishing the merged bytes makes them rune's newest publish, so
    /// undoing past the install afterward leaves only the buffer moved — an
    /// ordinary unsaved edit the user is free to save over.
    #[test]
    fn undo_past_an_install_that_was_published_is_buffer_ahead() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let saved_seq = merge_after_an_external_write(&tx, session_id, doc_id);
        publish_save(
            &tx,
            session_id,
            doc_id,
            "base-one-merged",
            "the merged save",
        );
        undo_to(&tx, session_id, doc_id, saved_seq);

        assert_eq!(
            verdict(&tx, session_id, doc_id),
            SyncKind::BufferAhead,
            "disk holds exactly what rune last published; undoing past it is an unsaved edit"
        );
        tx.commit().expect("commit");
    }

    /// Without that publish the disk still holds the external bytes and
    /// rune's newest publish says something else — undoing past the install
    /// withdraws the only acceptance those bytes ever had.
    #[test]
    fn undo_past_an_unpublished_install_stays_diverged() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let saved_seq = merge_after_an_external_write(&tx, session_id, doc_id);
        undo_to(&tx, session_id, doc_id, saved_seq);

        assert_eq!(
            verdict(&tx, session_id, doc_id),
            SyncKind::Diverged,
            "bytes nobody published stay a conflict once the acceptance is undone"
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

    /// Undoing past bytes rune itself published leaves the user's own save
    /// on disk and only the buffer moved — an ordinary unsaved edit, never a
    /// conflict against the user's own content.
    #[test]
    fn undo_past_our_own_plain_save_is_buffer_ahead() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        open_document(&tx, session_id, doc_id, "base");
        let anchor = crate::journal::current_seq(&tx, session_id, doc_id).expect("current_seq");
        type_text(&tx, session_id, doc_id, "-more");
        publish_save(&tx, session_id, doc_id, "base-more", "the save");
        undo_to(&tx, session_id, doc_id, anchor);

        assert_eq!(
            verdict(&tx, session_id, doc_id),
            SyncKind::BufferAhead,
            "disk holds bytes rune published; undoing past them must not offer a merge against our own content"
        );
        tx.commit().expect("commit");
    }
}
