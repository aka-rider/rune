//! Tests for `load`/`load_from_read` — split out to keep the parent under
//! the file-size ceiling, the same shape `writer_tests.rs` already uses
//! elsewhere in this crate.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use rune_core::buffer::AppliedEdit;
use rune_vfs::Mem;

fn open() -> Connection {
    crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(
        &crate::conn::memory_uri(),
    ))
    .expect("open")
}

fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

fn always_alive(_pid: i64, _started_at: &str) -> bool {
    true
}

fn always_dead(_pid: i64, _started_at: &str) -> bool {
    false
}

#[test]
fn first_load_anchors_a_snapshot_and_adopts() {
    let mut conn = open();
    let vfs = Mem::new();
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"hello world");

    let result = load(
        &mut conn,
        &vfs,
        session_id,
        &always_alive,
        path,
        SystemTime::now(),
    )
    .expect("load");
    assert_eq!(result.disk_content, "hello world");
    assert_eq!(result.recovered, "hello world");
    assert!(
        !result.has_history,
        "HasHistory must reflect PRIOR history only"
    );
    assert_eq!(result.sync.kind, crate::sync::SyncKind::Clean);

    let saved_obs_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session_documents WHERE session_id=?1 AND doc_id=?2)",
            params![session_id, result.doc_id],
            |r| r.get(0),
        )
        .expect("check");
    assert!(saved_obs_exists, "first load must adopt");
}

/// End-to-end through the full [`load`] entry point (finding 8): a
/// fresh session inheriting a dead session's draft must report the
/// bridge edit's own durable seq in `LoadResult::bridge_seq`, exactly
/// matching this session's own `current_seq` immediately after —
/// a caller seeding `last_known_seq` from a hardcoded `0` instead would
/// silently regress behind a `move_undo_pos`/`materialize` issued
/// before this session's first ordinary `AppendEdit` ack lands.
#[test]
fn load_through_inheritance_reports_the_bridge_edits_own_durable_seq() {
    let mut conn = open();
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"shared content");

    // Session A loads, types an unsaved edit, then "dies" without
    // saving.
    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    let doc_id = load(
        &mut conn,
        &vfs,
        session_a,
        &always_alive,
        path,
        SystemTime::now(),
    )
    .expect("session a load")
    .doc_id;
    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_a,
            SystemTime::now(),
            doc_id,
            &[AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "UNSAVED ".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit");
        tx.commit().expect("commit");
    }

    // Session B (a fresh process/session) loads the SAME doc after A
    // died — disk hasn't moved since A's own baseline, so B inherits
    // A's unsaved content via a bridge edit.
    let session_b = crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
    let result = load(
        &mut conn,
        &vfs,
        session_b,
        &always_dead,
        path,
        SystemTime::now(),
    )
    .expect("session b load");

    assert_eq!(result.recovered, "UNSAVED shared content");
    let bridge_seq = result
        .bridge_seq
        .expect("a bridge edit must have been journaled for session b");

    let head = retry::with_retry(&mut conn, |tx| {
        crate::journal::current_seq(tx, session_b, doc_id)
    })
    .expect("current_seq");
    assert_eq!(
        head, bridge_seq,
        "bridge_seq must equal this session's own durable journal head"
    );
}

/// The mirror-image control: no cross-session inheritance happened (a
/// document's very first-ever load, no prior session at all), so
/// `bridge_seq` must be `None` — never a stale or fabricated seq.
#[test]
fn load_without_inheritance_reports_no_bridge_seq() {
    let mut conn = open();
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"hello world");
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");

    let result = load(
        &mut conn,
        &vfs,
        session_id,
        &always_alive,
        path,
        SystemTime::now(),
    )
    .expect("load");
    assert_eq!(result.bridge_seq, None);
}

/// End-to-end DATA-LOSS regression through the full public [`load`]
/// entry point: session A opens (H0), edits (journaled durably, never
/// saved), the file is overwritten by an external ATOMIC SWAP (H1,
/// mints a new inode — `document::open_path_by_inode`'s reclaim branch,
/// B3), and session A dies without saving. Session B, a fresh process
/// reopening the same path, must re-anchor on A's own baseline (H0),
/// bridge H0 -> A's draft, and end up `Diverged` against disk's current
/// content (H1) — never silently dropping A's draft in favor of
/// whatever is on disk now.
#[test]
fn diverged_load_bridges_the_dead_sessions_own_baseline_not_disk() {
    use rune_vfs::Vfs;

    let mut conn = open();
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"session A's content");

    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    let doc_id = load(
        &mut conn,
        &vfs,
        session_a,
        &always_alive,
        path,
        SystemTime::now(),
    )
    .expect("session a load")
    .doc_id;
    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_a,
            SystemTime::now(),
            doc_id,
            &[AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "UNSAVED ".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit");
        tx.commit().expect("commit");
    }

    // An external atomic-swap overwrite — same path, a NEW inode.
    vfs.save_atomic(path, b"disk moved on independently")
        .expect("external atomic swap");

    let session_b = crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
    let result = load(
        &mut conn,
        &vfs,
        session_b,
        &always_dead,
        path,
        SystemTime::now(),
    )
    .expect("session b load");

    assert_eq!(
        result.doc_id, doc_id,
        "the swap must reuse A's document row"
    );
    assert_eq!(
        result.recovered, "UNSAVED session A's content",
        "must bridge from A's own baseline, never silently drop A's draft"
    );
    assert_eq!(result.sync.kind, crate::sync::SyncKind::Diverged);

    let bridge_seq = result
        .bridge_seq
        .expect("a bridge edit must have been journaled for session b");
    let head = retry::with_retry(&mut conn, |tx| {
        crate::journal::current_seq(tx, session_b, doc_id)
    })
    .expect("current_seq");
    assert_eq!(head, bridge_seq);

    let saved_obs = retry::with_retry(&mut conn, |tx| {
        observation::saved_obs_for(tx, session_b, doc_id)
    })
    .expect("saved_obs_for")
    .expect("session b adopted a baseline");
    assert_eq!(
        saved_obs.blob_hash.as_str(),
        observation::hash_bytes(b"session A's content"),
        "saved_obs (CAS baseline) must be A's own H0, not disk's H1"
    );
}

/// End-to-end regression pinning the user-facing bug: session A opens a
/// file and never edits it; A dies; the file is rewritten externally;
/// session B loads it. B must get the current disk content as a
/// non-divergent adoption (A never had anything unsaved to inherit),
/// AND B's own journal reconstruction must equal what B's buffer holds
/// — the assertion that would have caught a returned string not backed
/// by a matching journal entry.
#[test]
fn dead_session_with_no_edit_yields_disk_and_the_new_sessions_journal_agrees() {
    use rune_vfs::Vfs;

    let mut conn = open();
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"original content");

    let session_a = crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
    load(
        &mut conn,
        &vfs,
        session_a,
        &always_alive,
        path,
        SystemTime::now(),
    )
    .expect("session a load");

    vfs.save_atomic(path, b"rewritten externally")
        .expect("external atomic swap");

    let session_b = crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
    let result = load(
        &mut conn,
        &vfs,
        session_b,
        &always_dead,
        path,
        SystemTime::now(),
    )
    .expect("session b load");

    assert_eq!(result.disk_content, "rewritten externally");
    assert_eq!(
        result.recovered, "rewritten externally",
        "a session that never edited leaves nothing to inherit"
    );
    assert_ne!(
        result.sync.kind,
        crate::sync::SyncKind::Diverged,
        "nothing unsaved means a clean adoption, not a divergence"
    );

    let reconstructed = retry::with_retry(&mut conn, |tx| {
        crate::snapshot::recover_document(tx, session_b, result.doc_id)
    })
    .expect("recover_document");
    assert_eq!(
        reconstructed, result.recovered,
        "session b's own journal must reconstruct to exactly what its buffer holds"
    );
}

/// TOCTOU pin: `load_from_read` adopts whatever bytes the caller's own
/// taken read carries, never a second, independent read of the same path —
/// the CAS baseline and the returned content both trace to the SAME
/// sighting even when disk has moved on since that sighting was taken.
#[test]
fn load_from_read_adopts_the_taken_bytes_never_a_fresh_disk_read() {
    let mut conn = open();
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"taken bytes");

    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let read = bracket::bracketed_read(&vfs, path).expect("bracketed_read");

    // Disk moves on AFTER the read was taken but BEFORE `load_from_read`
    // ever runs — a real TOCTOU window.
    vfs.save_atomic(path, b"current disk content, never adopted")
        .expect("external rewrite after the read was taken");

    let result = load_from_read(
        &mut conn,
        &vfs,
        session_id,
        &always_alive,
        path,
        read,
        SystemTime::now(),
    )
    .expect("load_from_read");

    assert_eq!(
        result.disk_content, "taken bytes",
        "the buffer must be exactly the taken read's bytes, never a fresh disk read"
    );
    let saved_obs = retry::with_retry(&mut conn, |tx| {
        observation::saved_obs_for(tx, session_id, result.doc_id)
    })
    .expect("saved_obs_for")
    .expect("first load must adopt a baseline");
    assert_eq!(
        saved_obs.blob_hash.as_str(),
        observation::hash_bytes(b"taken bytes"),
        "the CAS baseline must be the taken read's own hash, never the current disk hash"
    );
}

/// `load_from_read` with an unconfirmed taken read records the resulting
/// observation as unconfirmed — mirroring `bracket.rs`'s own churn tests
/// (`an_unstable_bracket_reports_unconfirmed`), now exercised through the
/// full [`load_from_read`] entry point rather than the bracket alone.
#[test]
fn load_from_read_with_an_unconfirmed_read_records_an_unconfirmed_observation() {
    let mut conn = open();
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"churning content");
    vfs.set_churning(path, true);

    let read = bracket::bracketed_read(&vfs, path).expect("bracketed_read");
    assert!(
        !read.confirmed,
        "the churning vfs must yield an unstable bracket"
    );

    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let result = load_from_read(
        &mut conn,
        &vfs,
        session_id,
        &always_alive,
        path,
        read,
        SystemTime::now(),
    )
    .expect("load_from_read");

    let recorded_confirmed: Option<bool> = conn
        .query_row(
            "SELECT confirmed FROM observations WHERE doc_id=?1 ORDER BY id DESC LIMIT 1",
            params![result.doc_id],
            |r| r.get(0),
        )
        .expect("read back the recorded observation");
    assert_eq!(
        recorded_confirmed,
        Some(false),
        "an unconfirmed taken read must record an unconfirmed observation"
    );
}
