//! Crash recovery, GC contention, and divergence scenarios.

use std::sync::Arc;
use std::sync::mpsc;

use rusqlite::params;

use rune_core::buffer::AppliedEdit;
use rune_db::{DbEvent, OnEvent, Store};

use crate::support::{
    MARKER_SAFETY_DEADLINE, seed_schema_and_docs, spawn_helper, temp_dir, touch,
    wait_ready_or_child_death,
};

#[test]
fn child_sigkilled_mid_storm_recovers_at_last_committed_batch_and_reaper_reclaims() {
    let dir = temp_dir("kill-mid-storm");
    let path = dir.join("rune-v1.db");
    let doc_ids = seed_schema_and_docs(&path, 1);
    let doc_id = rune_db::DocId(doc_ids[0]);
    let count = 200usize;
    let checkpoint = 50usize;

    let session_marker = dir.join("session");
    let checkpoint_marker = dir.join("checkpoint");
    let release_marker = dir.join("release");

    let mut children = vec![spawn_helper(
        "append_storm_checkpoint",
        &[
            ("RUNE_DB_PATH", path.display().to_string()),
            ("RUNE_DB_DOC_ID", doc_id.to_string()),
            ("RUNE_DB_COUNT", count.to_string()),
            ("RUNE_DB_CHECKPOINT", checkpoint.to_string()),
            (
                "RUNE_DB_SESSION_MARKER",
                session_marker.display().to_string(),
            ),
            (
                "RUNE_DB_CHECKPOINT_MARKER",
                checkpoint_marker.display().to_string(),
            ),
            (
                "RUNE_DB_RELEASE_MARKER",
                release_marker.display().to_string(),
            ),
        ],
    )];

    wait_ready_or_child_death(
        &mut children,
        std::slice::from_ref(&checkpoint_marker),
        MARKER_SAFETY_DEADLINE,
    );
    let mut child = children.pop().expect("checkpoint child present");
    child.kill().expect("sigkill child");
    let _ = child.wait_with_output();

    let killed_session_id = rune_db::SessionId(
        std::fs::read_to_string(&session_marker)
            .expect("read session marker")
            .trim()
            .parse()
            .expect("parse session id"),
    );

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let (store, warning) =
        Store::open(&path, Arc::new(rune_vfs::Disk), on_event).expect("reopen store");
    assert!(warning.is_none());
    assert!(!store.degraded());

    let mut verify =
        rune_db::open_raw_connection_at_path_for_test(&path).expect("verify connection");
    let recovered = {
        let tx = verify.transaction().expect("tx");
        let content = rune_db::recover_document(&tx, killed_session_id, doc_id)
            .expect("recover_document")
            .content;
        tx.commit().expect("commit");
        content
    };
    let expected: String = (0..checkpoint).rev().map(|i| format!("{i} ")).collect();
    assert_eq!(
        recovered, expected,
        "recovered content must match exactly the committed prefix"
    );

    let killed_events: i64 = verify
        .query_row(
            "SELECT COUNT(*) FROM events WHERE session_id=?1",
            params![killed_session_id],
            |r| r.get(0),
        )
        .expect("count killed session events");
    assert_eq!(
        killed_events, checkpoint as i64,
        "exactly `checkpoint` events must have committed before the kill"
    );

    let edit = AppliedEdit {
        start: recovered.len(),
        end: recovered.len(),
        deleted: String::new(),
        insert: "NEW".to_string(),
    };
    let id = store
        .append_edit(
            doc_id,
            rune_db::BindingToken::next(),
            rune_db::Seq(0),
            &[edit],
            &[],
            &[],
        )
        .expect("enqueue append");
    match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok { id: got, .. }) if got == id => {}
        other => panic!("expected append ack, got {other:?}"),
    }
    store.shutdown();

    let mut reap_conn =
        rune_db::open_recovery_store_at_path_for_test(&path).expect("reap connection");
    rune_db::reap_dead_sessions(&mut reap_conn, &rune_db::is_process_alive, None).expect("reap");

    let killed_events_after_reap: i64 = reap_conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE session_id=?1",
            params![killed_session_id],
            |r| r.get(0),
        )
        .expect("count killed session events after reap");
    assert_eq!(
        killed_events_after_reap, 0,
        "the killed, now-superseded session's footprint must be reaped"
    );

    let killed_session_row_exists: bool = reap_conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
            params![killed_session_id],
            |r| r.get(0),
        )
        .expect("check sessions row");
    assert!(
        !killed_session_row_exists,
        "an observation-free sessions row must be reaped alongside its footprint"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gc_contention_never_drops_a_write_or_leaves_a_dangling_blob_reference() {
    let dir = temp_dir("gc-contention");
    let path = dir.join("rune-v1.db");
    let doc_ids = seed_schema_and_docs(&path, 1);
    let doc_id = doc_ids[0];
    let count = 20usize;
    let go = dir.join("go");
    let editor_ready = dir.join("editor-ready");
    let sweeper_ready = dir.join("sweeper-ready");

    let editor = spawn_helper(
        "gc_editor",
        &[
            ("RUNE_DB_PATH", path.display().to_string()),
            ("RUNE_DB_DOC_ID", doc_id.to_string()),
            ("RUNE_DB_COUNT", count.to_string()),
            ("RUNE_DB_READY_MARKER", editor_ready.display().to_string()),
            ("RUNE_DB_GO_MARKER", go.display().to_string()),
        ],
    );
    let sweeper = spawn_helper(
        "gc_sweeper",
        &[
            ("RUNE_DB_PATH", path.display().to_string()),
            ("RUNE_DB_COUNT", count.to_string()),
            ("RUNE_DB_READY_MARKER", sweeper_ready.display().to_string()),
            ("RUNE_DB_GO_MARKER", go.display().to_string()),
        ],
    );

    let mut children = vec![editor, sweeper];
    wait_ready_or_child_death(
        &mut children,
        &[editor_ready.clone(), sweeper_ready.clone()],
        MARKER_SAFETY_DEADLINE,
    );
    touch(&go);

    let mut children = children.into_iter();
    for label in ["gc_editor", "gc_sweeper"] {
        let child = children.next().expect("child present");
        let output = child.wait_with_output().expect("wait child");
        assert!(
            output.status.success(),
            "{label} child failed under concurrent GC contention: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let verify =
        rune_db::open_raw_connection_at_path_for_test(&path).expect("open verify connection");
    let dangling: i64 = verify
        .query_row(
            "SELECT COUNT(*) FROM ( \
                SELECT blob_hash FROM snapshots    WHERE blob_hash NOT IN (SELECT hash FROM blobs) \
                UNION ALL \
                SELECT blob_hash FROM observations WHERE blob_hash NOT IN (SELECT hash FROM blobs) \
             )",
            [],
            |r| r.get(0),
        )
        .expect("count dangling blob references");
    assert_eq!(
        dangling, 0,
        "no live snapshot/observation may reference a blob a concurrent sweep removed"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reopen_after_external_atomic_swap_bridges_the_dead_sessions_own_draft() {
    use rune_vfs::VfsTestExt;

    let dir = temp_dir("reopen-dataloss");
    let path = dir.join("rune-v1.db");
    let doc_path = dir.join("doc.md");
    std::fs::write(&doc_path, b"session A's content").expect("seed doc file");

    let doc_id_marker = dir.join("doc-id");
    let child = spawn_helper(
        "edit_and_die",
        &[
            ("RUNE_DB_PATH", path.display().to_string()),
            ("RUNE_DB_DOC_PATH", doc_path.display().to_string()),
            ("RUNE_DB_DOC_ID_MARKER", doc_id_marker.display().to_string()),
        ],
    );
    let output = child.wait_with_output().expect("wait child");
    assert!(
        output.status.success(),
        "edit_and_die child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let doc_id: i64 = std::fs::read_to_string(&doc_id_marker)
        .expect("read doc id marker")
        .trim()
        .parse()
        .expect("parse doc id");

    rune_vfs::Disk
        .save_atomic(&doc_path, b"disk moved on independently")
        .expect("external atomic swap");

    let recovered_marker = dir.join("recovered");
    let sync_marker = dir.join("sync-kind");
    let child2 = spawn_helper(
        "reload_diverged",
        &[
            ("RUNE_DB_PATH", path.display().to_string()),
            ("RUNE_DB_DOC_PATH", doc_path.display().to_string()),
            (
                "RUNE_DB_RECOVERED_MARKER",
                recovered_marker.display().to_string(),
            ),
            ("RUNE_DB_SYNC_MARKER", sync_marker.display().to_string()),
        ],
    );
    let output2 = child2.wait_with_output().expect("wait child");
    assert!(
        output2.status.success(),
        "reload_diverged child failed: {}",
        String::from_utf8_lossy(&output2.stderr)
    );

    let recovered = std::fs::read_to_string(&recovered_marker).expect("read recovered marker");
    assert_eq!(
        recovered, "UNSAVED session A's content",
        "the dead session's draft must survive a real cross-process reopen after an \
         external atomic swap, never silently dropped"
    );
    let sync_kind = std::fs::read_to_string(&sync_marker).expect("read sync marker");
    assert_eq!(sync_kind.trim(), "Diverged");

    let verify =
        rune_db::open_raw_connection_at_path_for_test(&path).expect("open verify connection");
    let doc_rows: i64 = verify
        .query_row(
            "SELECT COUNT(*) FROM documents WHERE id=?1",
            params![doc_id],
            |r| r.get(0),
        )
        .expect("count doc rows");
    assert_eq!(doc_rows, 1, "the swap must reuse the same document row");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_second_processs_real_save_is_a_real_divergence_for_merge_prep() {
    let dir = temp_dir("second-instance-save");
    let path = dir.join("rune-v1.db");
    let doc_path = dir.join("doc.md");
    std::fs::write(&doc_path, b"one\n").expect("seed doc file");

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let (store, warning) =
        Store::open(&path, Arc::new(rune_vfs::Disk), on_event).expect("open session A's store");
    assert!(warning.is_none());

    let id = store.load(&doc_path).expect("enqueue session A's load");
    let doc_id = match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::Load(result),
        }) if got == id => result.doc_id,
        other => panic!("expected session A's load ack, got {other:?}"),
    };

    let edit = AppliedEdit {
        start: 0,
        end: 0,
        deleted: String::new(),
        insert: "A ".to_string(),
    };
    let id = store
        .append_edit(
            doc_id,
            rune_db::BindingToken::next(),
            rune_db::Seq(0),
            &[edit],
            &[],
            &[],
        )
        .expect("enqueue session A's edit");
    match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::Seq(_),
        }) if got == id => {}
        other => panic!("expected session A's append ack, got {other:?}"),
    }

    let doc_id_marker = dir.join("doc-id");
    let child = spawn_helper(
        "save_and_die",
        &[
            ("RUNE_DB_PATH", path.display().to_string()),
            ("RUNE_DB_DOC_PATH", doc_path.display().to_string()),
            ("RUNE_DB_DOC_ID_MARKER", doc_id_marker.display().to_string()),
            ("RUNE_DB_INSERT", "B ".to_string()),
        ],
    );
    let output = child.wait_with_output().expect("wait for child B");
    assert!(
        output.status.success(),
        "save_and_die child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let child_doc_id: i64 = std::fs::read_to_string(&doc_id_marker)
        .expect("read child doc id marker")
        .trim()
        .parse()
        .expect("parse child doc id");
    assert_eq!(
        child_doc_id, doc_id.0,
        "both processes must resolve the same document row for the same path"
    );

    let id = store
        .merge_prep(doc_id)
        .expect("enqueue session A's merge_prep");
    let prep = match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::MergePrep(prep),
        }) if got == id => *prep,
        other => panic!("expected session A's merge_prep ack, got {other:?}"),
    };

    assert!(
        prep.sync.kind.is_disk_divergent(),
        "session A's own unsaved edit plus session B's real cross-process save must classify \
         disk-divergent, got {:?}",
        prep.sync.kind
    );
    let rune_db::MergePrepOutcome::Ready { theirs, .. } = prep.outcome else {
        panic!("expected Ready, got {:?}", prep.outcome);
    };
    let (_, theirs_bytes) = theirs.expect("theirs must be present");
    assert_eq!(
        theirs_bytes,
        b"B one\n".to_vec(),
        "theirs must be session B's own real save, not session A's own baseline"
    );

    store.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
