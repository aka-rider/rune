//! Unit tests for `materialize.rs`, kept in a sibling file so that module
//! itself stays inside the 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use rune_vfs::{Mem, Vfs};

use super::*;
use crate::ids::{DocId, SessionId};
use crate::test_support::open;

fn seed_doc_with_path(conn: &Connection, path: &str) -> DocId {
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES (?1, 'x', 'x')",
        params![path],
    )
    .expect("seed doc");
    DocId(conn.last_insert_rowid())
}

fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

/// Seeds an `observations` row whose CAS baseline is the hash of
/// `content` — `blob_hash` is FK-constrained to `blobs.hash`, so
/// `content` is durably stored first via `put_blob` (never a
/// hand-picked hash string with no backing row).
fn record_obs(conn: &Connection, doc_id: DocId, session_id: SessionId, content: &str) -> ObsId {
    let hash = crate::blob::put_blob(conn, content.as_bytes()).expect("seed blob");
    conn.execute(
        "INSERT INTO observations(doc_id, session_id, blob_hash, seq, size, mtime, origin, at) \
         VALUES(?1,?2,?3,NULL,0,'t','load','t')",
        params![doc_id, session_id, hash],
    )
    .expect("seed observation");
    crate::ids::obs_id_from_rowid(conn.last_insert_rowid()).expect("nonzero")
}

fn stat_of(vfs: &Mem, path: &Path) -> StatFacts {
    observation::stat_identity(vfs, path)
}

/// [`prepare_materialize`] is pure DB bookkeeping — it must never touch
/// `vfs` at all. Proven by never constructing a `Vfs` in this test:
/// there is nothing for it to call even if it tried.
#[test]
fn prepare_materialize_is_vfs_free_and_returns_the_bound_path_and_expect_hash() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let doc_id = seed_doc_with_path(&conn, "/doc.md");
    let expect = record_obs(&conn, doc_id, session_id, "original");

    let prep = prepare_materialize(
        &mut conn,
        DocSession { doc_id, session_id },
        MaterializeTarget::Existing { expect },
    )
    .expect("prepare");
    let (bound_path, expect_hash) = match prep {
        MaterializePrep::Overwrite {
            bound_path,
            expect_hash,
            ..
        } => (bound_path, expect_hash),
        MaterializePrep::Create => unreachable!("expected Overwrite, got Create"),
    };
    assert_eq!(bound_path, "/doc.md");
    assert_eq!(
        expect_hash,
        crate::ids::BlobHash(observation::hash_bytes(b"original"))
    );
}

/// `BindNew` skips the bound-path/CAS-baseline lookup entirely —
/// `materialize_create`'s original shape never consulted `expect`.
#[test]
fn prepare_materialize_create_target_is_a_pure_no_query() {
    let mut conn = open();
    // No document row at all — if `prepare_materialize` tried to read
    // one for `BindNew`, this would error instead of returning
    // cleanly.
    let prep = prepare_materialize(
        &mut conn,
        DocSession {
            doc_id: DocId(999),
            session_id: SessionId(999),
        },
        MaterializeTarget::BindNew,
    )
    .expect("prepare BindNew");
    assert_eq!(prep, MaterializePrep::Create);
}

/// An untitled document (empty bound path) must refuse rather than
/// silently proceeding — the caller has nothing to CAS its target
/// against.
#[test]
fn prepare_materialize_refuses_an_untitled_document() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let doc_id = seed_doc_with_path(&conn, "");
    let expect = record_obs(&conn, doc_id, session_id, "irrelevant");

    let err = prepare_materialize(
        &mut conn,
        DocSession { doc_id, session_id },
        MaterializeTarget::Existing { expect },
    )
    .expect_err("untitled document must refuse");
    assert!(matches!(err, Error::Invalid(_)));
}

/// A CAS conflict (the caller's own vfs read/hash-compare found the
/// live target disagreeing with `expect`) records the live bytes as a
/// fresh, `origin='probe'` observation and never marks the write
/// committed — using ONLY caller-supplied bytes/stat, no `vfs` call.
#[test]
fn record_materialize_outcome_conflict_records_fresh_and_never_commits() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"external content");
    let doc_id = seed_doc_with_path(&conn, "/doc.md");
    let stat = stat_of(&vfs, path);

    let result = record_materialize_outcome(
        &mut conn,
        DocSession { doc_id, session_id },
        Path::new("/doc.md"),
        1,
        SystemTime::now(),
        MaterializeOutcome::Conflict {
            data: b"external content".to_vec(),
            origin: ObsOrigin::Probe,
            stat,
            confirmed: true,
        },
    )
    .expect("record conflict");

    let MatResult::Refused { fresh } = result else {
        unreachable!("expected Refused");
    };
    assert_eq!(fresh.origin, ObsOrigin::Probe);
    assert_eq!(
        fresh.blob_hash.as_str(),
        observation::hash_bytes(b"external content")
    );
    assert_eq!(fresh.confirmed, Confirmation::Confirmed);
}

/// A conflict capture the caller's own bracket could not confirm (a
/// racer caught mid-external-rewrite) must record `confirmed: false` —
/// never `None`/unclassified — so this observation can never later be
/// picked up by the probe short-circuit or served as a merge Theirs.
#[test]
fn record_materialize_outcome_conflict_unconfirmed_records_confirmed_false() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"external content");
    let doc_id = seed_doc_with_path(&conn, "/doc.md");
    let stat = stat_of(&vfs, path);

    let result = record_materialize_outcome(
        &mut conn,
        DocSession { doc_id, session_id },
        Path::new("/doc.md"),
        1,
        SystemTime::now(),
        MaterializeOutcome::Conflict {
            data: b"external content".to_vec(),
            origin: ObsOrigin::Probe,
            stat,
            confirmed: false,
        },
    )
    .expect("record conflict");

    let MatResult::Refused { fresh } = result else {
        unreachable!("expected Refused");
    };
    assert_eq!(fresh.confirmed, Confirmation::Unconfirmed);
}

/// A plain committed write records the blob/observation and rebinds the
/// document's row to the resolved path — the same effect `commit_save`
/// always had, now driven by caller-supplied facts only.
#[test]
fn record_materialize_outcome_committed_records_save_and_rebinds() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"original");
    let doc_id = seed_doc_with_path(&conn, "/doc.md");
    let stat = stat_of(&vfs, path);

    let result = record_materialize_outcome(
        &mut conn,
        DocSession { doc_id, session_id },
        Path::new("/doc.md"),
        7,
        SystemTime::now(),
        MaterializeOutcome::Committed {
            data: b"new content".to_vec(),
            stat,
            confirmed: true,
        },
    )
    .expect("record committed");

    let MatResult::Committed { saved } = result else {
        unreachable!("expected Committed");
    };
    let saved = saved.expect("saved observation recorded");
    assert_eq!(saved.origin, ObsOrigin::Save);
    assert_eq!(saved.seq, Some(7));
    assert_eq!(
        saved.blob_hash.as_str(),
        observation::hash_bytes(b"new content")
    );
    assert_eq!(saved.confirmed, Confirmation::Confirmed);

    let bound_path: String = conn
        .query_row(
            "SELECT path FROM documents WHERE id=?1",
            params![doc_id],
            |r| r.get(0),
        )
        .expect("read back bound path");
    assert_eq!(bound_path, "/doc.md");
}

/// A swap-race outcome records BOTH the displaced bytes (`origin=
/// 'swap'`) and our own committed write — unconditional displaced-bytes
/// capture, driven entirely by caller-supplied facts.
#[test]
fn record_materialize_outcome_raced_records_both_displaced_and_committed() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"our content");
    let doc_id = seed_doc_with_path(&conn, "/doc.md");
    let stat = stat_of(&vfs, path);

    let result = record_materialize_outcome(
        &mut conn,
        DocSession { doc_id, session_id },
        Path::new("/doc.md"),
        3,
        SystemTime::now(),
        MaterializeOutcome::Raced {
            data: b"our content".to_vec(),
            stat,
            confirmed: true,
            displaced: b"racer bytes".to_vec(),
            displaced_stat: StatFacts::default(),
        },
    )
    .expect("record raced");

    let MatResult::CommittedRaced { saved, displaced } = result else {
        unreachable!("expected CommittedRaced");
    };
    assert_eq!(displaced.origin, ObsOrigin::Swap);
    assert_eq!(
        displaced.blob_hash.as_str(),
        observation::hash_bytes(b"racer bytes")
    );
    assert_eq!(
        saved.blob_hash.as_str(),
        observation::hash_bytes(b"our content")
    );

    let blob = retry::with_retry(&mut conn, |tx| {
        crate::blob::get_blob(tx, displaced.blob_hash.as_str())
    })
    .expect("racer bytes durably stored as a blob");
    assert_eq!(blob, b"racer bytes");
}

/// Displaced bytes are captured unconditionally — even when they are
/// NOT valid UTF-8 (a binary file, or another process's own
/// in-progress non-text write landing in the swap window).
#[test]
fn record_materialize_outcome_conflict_with_non_utf8_bytes_captures_them_byte_exact() {
    let mut conn = open();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let doc_id = seed_doc_with_path(&conn, "/doc.md");
    let racer_bytes: &[u8] = &[0xff, 0xfe, 0x00, 0x9f, 0x92, 0x96, 0x80];

    let result = record_materialize_outcome(
        &mut conn,
        DocSession { doc_id, session_id },
        Path::new("/doc.md"),
        1,
        SystemTime::now(),
        MaterializeOutcome::Conflict {
            data: racer_bytes.to_vec(),
            origin: ObsOrigin::Swap,
            stat: StatFacts::default(),
            confirmed: false,
        },
    )
    .expect("record conflict with non-utf8 bytes");

    let MatResult::Refused { fresh } = result else {
        unreachable!("expected Refused");
    };
    let blob = retry::with_retry(&mut conn, |tx| {
        crate::blob::get_blob(tx, fresh.blob_hash.as_str())
    })
    .expect("non-utf8 bytes must still be durably stored as a blob");
    assert_eq!(blob, racer_bytes);
}
