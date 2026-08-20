#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    dead_code
)]

use super::*;
use crate::test_support::open;
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

fn merge_after_an_external_write(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
) -> Seq {
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

#[test]
fn undo_past_an_install_that_was_published_is_buffer_ahead() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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

#[test]
fn undo_past_an_unpublished_install_stays_diverged() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let tx = conn.transaction().expect("tx");
    let doc_id = seed_doc(&tx);

    let state = sync(&tx, session_id, doc_id).expect("sync");
    assert_eq!(state.kind, SyncKind::Clean);
    tx.commit().expect("commit");
}

#[test]
fn untitled_nonempty_buffer_with_no_disk_fact_is_buffer_ahead() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let tx = conn.transaction().expect("tx");
    let doc_id = seed_doc(&tx);
    let hash = crate::blob::put_blob(&tx, b"some content").expect("seed blob");

    crate::observation::record_observation(
        &tx,
        doc_id,
        session_id,
        crate::observation::ObservationMeta {
            blob_hash: &hash,
            seq: None,
            origin: ObsOrigin::Watch,
            confirmed: Confirmation::Unclassified,
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

fn seed_resolve_at_head(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
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
            origin: ObsOrigin::Load,
            confirmed: Confirmation::Unclassified,
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
            origin: ObsOrigin::Resolve,
            confirmed: Confirmation::Unclassified,
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

#[test]
fn resolve_at_head_seq_classifies_buffer_ahead_not_diverged() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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

#[test]
fn resolve_at_head_seq_matching_reconstruction_is_clean() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let tx = conn.transaction().expect("tx");
    let doc_id = seed_doc(&tx);
    seed_resolve_at_head(&tx, session_id, doc_id, b"original", b"merged");

    let state = sync(&tx, session_id, doc_id).expect("sync");
    assert_eq!(state.kind, SyncKind::Clean);
    tx.commit().expect("commit");
}

#[test]
fn edit_after_resolve_still_classifies_buffer_ahead() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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

#[test]
fn undo_past_our_own_plain_save_is_buffer_ahead() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
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
