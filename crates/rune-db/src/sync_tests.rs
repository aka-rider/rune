#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    dead_code
)]

use super::*;
use crate::journal_append::EditBatch;
use crate::test_support::open;
use rune_core::undo::EditKind;
use std::time::SystemTime;

fn seed_doc(tx: &Transaction<'_>) -> DocId {
    tx.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    DocId(tx.last_insert_rowid())
}

fn stat_of(content: &str, at: &str) -> crate::observation::StatFacts {
    crate::observation::StatFacts {
        size: Some(content.len() as i64),
        mtime: Some(at.to_string()),
        ..Default::default()
    }
}

fn open_document(tx: &Transaction<'_>, session_id: SessionId, doc_id: DocId, disk: &str) {
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
            seq: Some(seq.0),
            origin: ObsOrigin::Load,
            confirmed: Confirmation::Confirmed,
        },
        &stat_of(disk, "load"),
        "load",
        None,
    )
    .expect("record the load adoption");
}

fn type_text(tx: &Transaction<'_>, session_id: SessionId, doc_id: DocId, text: &str) -> Seq {
    let ours = crate::snapshot::recover_document(tx, session_id, doc_id)
        .expect("recover")
        .content;
    let end = ours.len();
    crate::journal::append_edit(
        tx,
        session_id,
        SystemTime::now(),
        doc_id,
        EditBatch {
            edits: &[rune_core::buffer::AppliedEdit {
                start: end,
                end,
                deleted: String::new(),
                insert: text.to_string(),
            }],
            cursors_before: &[],
            cursors_after: &[],
            kind: EditKind::Other,
        },
    )
    .expect("append_edit")
}

fn undo_to(tx: &Transaction<'_>, session_id: SessionId, doc_id: DocId, seq: Seq) {
    crate::journal::move_undo_pos(tx, session_id, doc_id, seq).expect("move_undo_pos");
}

fn publish_save(tx: &Transaction<'_>, session_id: SessionId, doc_id: DocId, bytes: &str, at: &str) {
    let hash = crate::blob::put_blob(tx, bytes.as_bytes()).expect("put the published blob");
    let seq = crate::journal::current_seq(tx, session_id, doc_id).expect("current_seq");
    crate::adopt::record_adoption_tx(
        tx,
        doc_id,
        session_id,
        crate::observation::ObservationMeta {
            blob_hash: &hash,
            seq: Some(seq.0),
            origin: ObsOrigin::Save,
            confirmed: Confirmation::Confirmed,
        },
        &stat_of(bytes, at),
        at,
        None,
    )
    .expect("record the save adoption");
}

fn external_write(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
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
            origin: ObsOrigin::Probe,
            confirmed: Confirmation::Confirmed,
        },
    )
    .expect("record the fresh disk sighting")
}

fn resolve_against(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    theirs: &crate::observation::Observation,
    edit_seq: Seq,
) {
    crate::adopt::record_adoption_tx(
        tx,
        doc_id,
        session_id,
        crate::observation::ObservationMeta {
            blob_hash: theirs.blob_hash.as_str(),
            seq: Some(edit_seq.0),
            origin: ObsOrigin::Resolve,
            confirmed: theirs.confirmed,
        },
        &theirs.stat(),
        "resolve",
        Some(theirs.id),
    )
    .expect("record the resolve adoption");
}

fn verdict(tx: &Transaction<'_>, session_id: SessionId, doc_id: DocId) -> SyncKind {
    sync(tx, session_id, doc_id).expect("sync").kind
}

#[test]
fn is_disk_divergent_is_true_only_for_disk_ahead_and_diverged() {
    assert!(SyncKind::DiskAhead.is_disk_divergent());
    assert!(SyncKind::Diverged.is_disk_divergent());
    assert!(
        !SyncKind::Clean.is_disk_divergent(),
        "no disk fact yet ahead of the buffer is not a divergence"
    );
    assert!(
        !SyncKind::BufferAhead.is_disk_divergent(),
        "an ordinary unsaved edit is the dirty flag's job, not a divergence"
    );
}

#[test]
fn anothers_sessions_newest_save_is_a_real_divergence() {
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

    let kind = verdict(&tx, session_id, doc_id);
    assert!(
        kind.is_disk_divergent(),
        "another session's save is a real divergence, not an authorization to overwrite, got {kind:?}"
    );
    tx.commit().expect("commit");
}

#[test]
fn our_own_sessions_newest_save_is_buffer_ahead() {
    let mut conn = open();
    let session_id =
        crate::session::establish_session(&conn, SystemTime::now()).expect("this session");
    let tx = conn.transaction().expect("tx");
    let doc_id = seed_doc(&tx);

    open_document(&tx, session_id, doc_id, "base");
    type_text(&tx, session_id, doc_id, "-mine");
    publish_save(&tx, session_id, doc_id, "base-theirs", "our own save");
    external_write(&tx, session_id, doc_id, "base-theirs", "rediscovery");

    assert_eq!(
        verdict(&tx, session_id, doc_id),
        SyncKind::BufferAhead,
        "bytes THIS session published are ours to overwrite"
    );
    tx.commit().expect("commit");
}

/// Bytes no `save` ever published belong to whoever put them there. A
/// confirmed read of them is knowledge, never authorization.
#[test]
fn a_fresh_sighting_rune_never_published_stays_diverged() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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
