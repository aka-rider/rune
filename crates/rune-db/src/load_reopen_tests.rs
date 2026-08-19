//! Same-session reopen tests for `load` — split out of `load_tests.rs` to
//! keep both files under the file-size ceiling, the same shape
//! `writer_tests.rs` already uses elsewhere in this crate.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::test_support::open;
use rune_core::buffer::AppliedEdit;
use rune_vfs::{Mem, Vfs, VfsTestExt};

fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

fn always_alive(_pid: i64, _started_at: &str) -> bool {
    true
}

/// The same-session mirror of
/// `dead_session_with_no_edit_yields_disk_and_the_new_sessions_journal_agrees`:
/// no second session, no crash — the SAME session loads a path, never edits
/// it, some other tool rewrites the file, and the same session reopens it
/// (e.g. a tab switch back onto an already-open document). Nothing of this
/// session's own is unsaved, so the reopen must adopt the new disk content
/// cleanly, and this session's own journal reconstruction must agree with
/// exactly what it returns.
#[test]
fn same_session_reopen_with_no_edit_adopts_the_new_disk_content() {
    let mut conn = open();
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"original content");

    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let doc_id = load(
        &mut conn,
        &vfs,
        session_id,
        &always_alive,
        path,
        SystemTime::now(),
    )
    .expect("first load")
    .doc_id;

    vfs.save_atomic(path, b"rewritten externally")
        .expect("external atomic swap");

    let result = load(
        &mut conn,
        &vfs,
        session_id,
        &always_alive,
        path,
        SystemTime::now(),
    )
    .expect("reopen");

    assert!(
        result.has_history,
        "the session's own prior load counts as history"
    );
    assert_eq!(result.disk_content, "rewritten externally");
    assert_eq!(
        result.recovered, "rewritten externally",
        "a session that never edited leaves nothing to inherit — the reopen must adopt disk"
    );
    assert_ne!(
        result.sync.kind,
        crate::sync::SyncKind::Diverged,
        "nothing unsaved means a clean adoption, not a divergence"
    );

    let reconstructed = retry::with_retry(&mut conn, |tx| {
        crate::snapshot::recover_document(tx, session_id, doc_id)
    })
    .expect("recover_document");
    assert_eq!(
        reconstructed, result.recovered,
        "this session's own journal must reconstruct to exactly what its buffer holds"
    );
}

/// The dirty-buffer counterpart: the same session reopens the same path
/// with unsaved edits of its own still in the buffer when disk is rewritten
/// externally. The unsaved edits must survive in the returned buffer,
/// the journal must stay consistent with what's returned, and `sync` must
/// classify `Diverged` so the DiskConflict guard engages downstream.
#[test]
fn same_session_reopen_with_unsaved_edits_stays_diverged() {
    let mut conn = open();
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"original content");

    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let doc_id = load(
        &mut conn,
        &vfs,
        session_id,
        &always_alive,
        path,
        SystemTime::now(),
    )
    .expect("first load")
    .doc_id;

    {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_id,
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

    vfs.save_atomic(path, b"rewritten externally")
        .expect("external atomic swap");

    let result = load(
        &mut conn,
        &vfs,
        session_id,
        &always_alive,
        path,
        SystemTime::now(),
    )
    .expect("reopen");

    assert_eq!(
        result.recovered, "UNSAVED original content",
        "unsaved edits must survive a reopen even when disk moved on"
    );
    assert_eq!(result.sync.kind, crate::sync::SyncKind::Diverged);

    let reconstructed = retry::with_retry(&mut conn, |tx| {
        crate::snapshot::recover_document(tx, session_id, doc_id)
    })
    .expect("recover_document");
    assert_eq!(
        reconstructed, result.recovered,
        "this session's own journal must reconstruct to exactly what its buffer holds"
    );
}
