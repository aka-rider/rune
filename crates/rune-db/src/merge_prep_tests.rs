#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::journal_append::EditBatch;
use crate::obs_origin::ObsOrigin;
use crate::test_support::open;
use rune_core::undo::EditKind;
use rune_vfs::{Mem, VfsTestExt};
use rusqlite::params;
use std::path::Path;

fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

/// Task WP-C(3): when this session's own CAS baseline was adopted with
/// no correlated seq (so `ancestor_at`'s session-scoped derivation can
/// never find it — `sync.ancestor` stays `None`), but that SAME
/// baseline is still reachable from the fresh `theirs` sighting via the
/// observations' own parent-edge lineage, the ladder's rung (i) must
/// still surface it rather than reporting absence.
#[test]
fn merge_prep_ancestor_ladder_prefers_lineage_over_an_absent_session_scoped_ancestor() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"baseline content");

    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    let doc_id = DocId(conn.last_insert_rowid());

    let stat = observation::StatFacts {
        size: Some(1),
        mtime: Some("t".to_string()),
        ..Default::default()
    };
    let hash_baseline = {
        let tx = conn.transaction().expect("tx");
        let h = crate::blob::put_blob(&tx, b"baseline content").expect("seed blob");
        tx.commit().expect("commit");
        h
    };
    // Adopted with NO correlated seq: `ancestor_at` requires `seq IS
    // NOT NULL`, so this baseline can never surface through the
    // session-scoped rung no matter the journal position.
    crate::adopt::record_adoption(
        &mut conn,
        doc_id,
        session_id,
        observation::ObservationMeta {
            blob_hash: &hash_baseline,
            seq: None,
            origin: ObsOrigin::Resolve,
            confirmed: Confirmation::Confirmed,
        },
        &stat,
        SystemTime::now(),
        None,
    )
    .expect("seed baseline adoption");

    let temp = vfs
        .write_durable(path, b"theirs content")
        .expect("write_durable");
    vfs.exchange(&temp, path).expect("exchange");
    let result =
        merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");

    assert_eq!(result.sync.kind, crate::sync::SyncKind::Diverged);
    assert_eq!(
        result.sync.ancestor, None,
        "the session-scoped rung must find nothing"
    );
    let MergePrepOutcome::Ready { ancestor, .. } = result.outcome else {
        unreachable!("expected Ready");
    };
    assert_eq!(
        ancestor,
        Some((AncestorRung::Lineage, b"baseline content".to_vec()))
    );
}

/// Plan WP3 "Done when" (a): a diverged fixture's `MergePrep` reports
/// `Diverged` and hands back both sides' actual bytes, not just hashes.
#[test]
fn merge_prep_on_a_diverged_document_returns_both_sides_bytes() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"theirs content");

    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    let doc_id = DocId(conn.last_insert_rowid());

    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &[rune_core::buffer::AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: "ours content".to_string(),
                }],
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append_edit");
        tx.commit().expect("commit");
    }

    let result =
        merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");
    assert_eq!(result.sync.kind, crate::sync::SyncKind::Diverged);
    let MergePrepOutcome::Ready { ancestor, theirs } = result.outcome else {
        unreachable!("expected Ready");
    };
    let (theirs_obs, theirs_bytes) = theirs.expect("theirs must be present");
    assert_eq!(theirs_bytes, b"theirs content".to_vec());
    let _ = theirs_obs;
    assert_eq!(ancestor, None, "no prior ancestor-eligible sighting");
}

/// Plan WP3 "Done when" (b): a `DiskAhead` document (clean buffer, disk
/// moved) returns the disk bytes as `theirs` with no ancestor divergence
/// story needed for the fast path.
#[test]
fn merge_prep_on_a_disk_ahead_document_returns_theirs_bytes() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"");

    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    let doc_id = DocId(conn.last_insert_rowid());

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
                origin: ObsOrigin::Load,
                confirmed: Confirmation::Unclassified,
            },
            &crate::observation::StatFacts {
                mtime: Some("t".to_string()),
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
    let MergePrepOutcome::Ready { theirs, .. } = result.outcome else {
        unreachable!("expected Ready");
    };
    let (_, theirs_bytes) = theirs.expect("theirs must be present");
    assert_eq!(theirs_bytes, b"disk moved on".to_vec());
}

/// Task WP-A(2ii): a persistently unconfirmed disk state (the file
/// keeps changing across every re-probe attempt) must never be served as
/// Theirs — `merge_prep` reports `MergePrepOutcome::Unstable`, never an
/// empty/unstable Theirs. Driven through `Mem::mutate_after_next_stat`,
/// re-armed after each of the bounded retry attempts so the bracket
/// never settles.
#[test]
fn merge_prep_reports_unstable_when_disk_keeps_disagreeing_with_itself() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"theirs content");

    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    let doc_id = DocId(conn.last_insert_rowid());

    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &[rune_core::buffer::AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: "ours content".to_string(),
                }],
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append_edit");
        tx.commit().expect("commit");
    }

    // Perpetual churn: the disk never stops moving, so no re-probe
    // attempt's own bracket can ever settle.
    vfs.set_churning(path, true);

    let result =
        merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");
    assert_eq!(
        result.outcome,
        MergePrepOutcome::Unstable,
        "a persistently unconfirmed disk must report unstable"
    );
}

/// Review fix F4: an untitled document (`path` is empty — `probe::probe`
/// degrades to a pure `sync::sync` with nothing to read from disk at
/// all) with no recorded observation has no `theirs` version — `Clean`
/// via `classify_sync`'s `theirs: None` branch. `theirs` comes back
/// `None` too, not an empty `Vec`/`0` sentinel standing in for "absent".
#[test]
fn merge_prep_on_an_untitled_document_returns_no_theirs() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");

    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
        [],
    )
    .expect("seed untitled doc");
    let doc_id = DocId(conn.last_insert_rowid());

    let result =
        merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");
    assert_eq!(result.sync.kind, crate::sync::SyncKind::Clean);
    let MergePrepOutcome::Ready { theirs, .. } = result.outcome else {
        unreachable!("expected Ready");
    };
    assert_eq!(theirs, None);
}

/// A legitimate external tool condensing a large file to a fraction of
/// its size, in one atomic publish (never a still-mutating churn), must
/// resolve within `merge_prep`'s own bounded re-probes rather than
/// staying `unstable` forever: the first internal probe sights the
/// shrink as an unconfirmed hypothesis, and the second — reading the
/// now-quiescent disk again — sees byte-identical content and confirms
/// it, so `merge_prep` serves the shrunk content as Theirs.
#[test]
fn merge_prep_serves_a_legitimate_shrink_confirmed_by_a_second_identical_sighting() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    let long_content = b"a very long paragraph of real disk content, unabridged";
    publish(&vfs, path, long_content);

    let loaded = crate::load::load(
        &mut conn,
        &vfs,
        session_id,
        &|_, _| false,
        path,
        SystemTime::now(),
    )
    .expect("load");
    let doc_id = loaded.doc_id;

    vfs.save_atomic(path, b"short").expect("publish shrink");

    let result =
        merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");

    assert_ne!(
        result.outcome,
        MergePrepOutcome::Unstable,
        "a stable, legitimate shrink must resolve, not stay unstable forever"
    );
    assert_eq!(result.sync.kind, crate::sync::SyncKind::DiskAhead);
    let MergePrepOutcome::Ready { theirs, .. } = result.outcome else {
        unreachable!("expected Ready");
    };
    let (_, theirs_bytes) = theirs.expect("theirs must be present");
    assert_eq!(theirs_bytes, b"short".to_vec());
}

/// Pins [`MERGE_PREP_MAX_ATTEMPTS`] as an exact ceiling, not merely an
/// approximate one: under permanent disk churn, `theirs_confirmed` never
/// settles, so EVERY re-probe attempt records one fresh `observations` row
/// (the stat short-circuit never fires on an unconfirmed prior fact —
/// `probe.rs`'s own module doc). `merge_prep` must make exactly
/// `MERGE_PREP_MAX_ATTEMPTS` probes — one before the loop plus
/// `MERGE_PREP_MAX_ATTEMPTS - 1` loop iterations — never one more: an
/// off-by-one relaxation of the bound (`<` loosened to `<=`) would let the
/// loop run a final, wasted probe past the documented ceiling.
#[test]
fn merge_prep_reprobes_exactly_the_configured_max_attempts_never_one_more() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"theirs content");

    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    let doc_id = DocId(conn.last_insert_rowid());

    vfs.set_churning(path, true);

    let obs_count = |conn: &Connection| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM observations WHERE doc_id=?1",
            params![doc_id],
            |r| r.get(0),
        )
        .expect("count observations")
    };
    let before = obs_count(&conn);

    let result =
        merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");
    assert_eq!(
        result.outcome,
        MergePrepOutcome::Unstable,
        "test setup: permanent churn must never confirm"
    );

    let after = obs_count(&conn);
    assert_eq!(
        after - before,
        i64::from(MERGE_PREP_MAX_ATTEMPTS),
        "must re-probe exactly MERGE_PREP_MAX_ATTEMPTS times, never one more"
    );
}
