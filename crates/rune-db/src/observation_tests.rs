#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

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

fn seed_blob(tx: &Transaction<'_>, content: &str) -> String {
    crate::blob::put_blob(tx, content.as_bytes()).expect("seed blob")
}

#[test]
fn get_observation_missing_id_is_an_error_not_a_default() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let tx = conn.transaction().expect("tx");
    let doc_id = seed_doc(&tx);
    let _ = session_id;
    let err =
        get_observation(&tx, ObsId::new(999).expect("nonzero")).expect_err("missing id must error");
    assert!(matches!(err, Error::NotFound(_)));
    let _ = doc_id;
}

#[test]
fn newest_observation_is_unscoped_across_sessions() {
    let mut conn = open();
    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    let session_b = crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
    let tx = conn.transaction().expect("tx");
    let doc_id = seed_doc(&tx);
    let hash_a = seed_blob(&tx, "content a");
    let hash_b = seed_blob(&tx, "content b");

    let stat = StatFacts {
        size: Some(1),
        mtime: Some("t".to_string()),
        ..Default::default()
    };
    record_observation(
        &tx,
        doc_id,
        session_a,
        ObservationMeta {
            blob_hash: &hash_a,
            seq: None,
            origin: ObsOrigin::Probe,
            confirmed: Confirmation::Unclassified,
        },
        &stat,
        "t",
    )
    .expect("record a");
    let b_id = record_observation(
        &tx,
        doc_id,
        session_b,
        ObservationMeta {
            blob_hash: &hash_b,
            seq: None,
            origin: ObsOrigin::Probe,
            confirmed: Confirmation::Unclassified,
        },
        &stat,
        "t",
    )
    .expect("record b");

    let newest = newest_observation(&tx, doc_id)
        .expect("newest")
        .expect("some");
    assert_eq!(newest.id, b_id, "newest must be session-unscoped");
    tx.commit().expect("commit");
}

#[test]
fn ancestor_at_self_reference_guard_excludes_only_at_exact_seq() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let tx = conn.transaction().expect("tx");
    let doc_id = seed_doc(&tx);
    let hash = seed_blob(&tx, "content");

    let obs_id = record_observation(
        &tx,
        doc_id,
        session_id,
        ObservationMeta {
            blob_hash: &hash,
            seq: Some(5),
            origin: ObsOrigin::Load,
            confirmed: Confirmation::Unclassified,
        },
        &StatFacts {
            size: Some(1),
            mtime: Some("t".to_string()),
            ..Default::default()
        },
        "t",
    )
    .expect("record");

    let at_same_pos = ancestor_at(&tx, doc_id, session_id, 5, Some(obs_id)).expect("query");
    assert!(at_same_pos.is_none(), "a fact cannot be its own ancestor");

    let at_later_pos = ancestor_at(&tx, doc_id, session_id, 6, Some(obs_id)).expect("query");
    assert!(
        at_later_pos.is_some(),
        "an older correlation for the same id is still a legitimate ancestor"
    );
    tx.commit().expect("commit");
}
