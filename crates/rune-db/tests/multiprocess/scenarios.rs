//! The four multiprocess scenarios: parent-side setup,
//! child spawning through `support::spawn_helper`, and post-mortem
//! verification against the shared db file every scenario's children raced
//! over.

use std::sync::Arc;
use std::sync::mpsc;

use rusqlite::{Connection, params};

use rune_core::buffer::AppliedEdit;
use rune_db::{DbEvent, OnEvent, Store};

use crate::support::{
    MARKER_SAFETY_DEADLINE, seed_schema_and_docs, spawn_helper, temp_dir, touch,
    wait_ready_or_child_death,
};

// ---------------------------------------------------------------------
// Scenario (a): 4 children append-storm one doc each concurrently
// ---------------------------------------------------------------------

#[test]
fn four_children_append_storm_one_doc_each_all_ack_ok_with_exact_event_counts() {
    let dir = temp_dir("append-storm");
    let path = dir.join("rune-v1.db");
    let doc_ids = seed_schema_and_docs(&path, 4);
    let count = 25usize;
    let go = dir.join("go");

    let mut children = Vec::new();
    let mut readies = Vec::new();
    for (i, doc_id) in doc_ids.iter().enumerate() {
        let ready = dir.join(format!("ready-{i}"));
        readies.push(ready.clone());
        children.push(spawn_helper(
            "append_storm",
            &[
                ("RUNE_DB_PATH", path.display().to_string()),
                ("RUNE_DB_DOC_ID", doc_id.to_string()),
                ("RUNE_DB_COUNT", count.to_string()),
                ("RUNE_DB_READY_MARKER", ready.display().to_string()),
                ("RUNE_DB_GO_MARKER", go.display().to_string()),
            ],
        ));
    }

    wait_ready_or_child_death(&mut children, &readies, MARKER_SAFETY_DEADLINE);
    touch(&go);

    for child in children {
        let output = child.wait_with_output().expect("wait child");
        assert!(
            output.status.success(),
            "append_storm child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let verify = Connection::open(&path).expect("open verify connection");
    for doc_id in &doc_ids {
        let n: i64 = verify
            .query_row(
                "SELECT COUNT(*) FROM events WHERE doc_id=?1",
                params![doc_id],
                |r| r.get(0),
            )
            .expect("count events");
        assert_eq!(
            n, count as i64,
            "doc {doc_id} must have exactly {count} events"
        );
    }
    let total: i64 = verify
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .expect("count total events");
    assert_eq!(total, 4 * count as i64);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------
// Scenario (b): two children race Store::open on a fresh path
// ---------------------------------------------------------------------

#[test]
fn two_children_race_store_open_on_a_fresh_path_apply_schema_once_both_get_sessions() {
    let dir = temp_dir("race-open");
    let path = dir.join("rune-v1.db"); // does NOT exist yet
    let go = dir.join("go");

    let mut children = Vec::new();
    let mut readies = Vec::new();
    for i in 0..2 {
        let ready = dir.join(format!("ready-{i}"));
        let opened = dir.join(format!("opened-{i}"));
        readies.push(ready.clone());
        children.push(spawn_helper(
            "race_open",
            &[
                ("RUNE_DB_PATH", path.display().to_string()),
                ("RUNE_DB_READY_MARKER", ready.display().to_string()),
                ("RUNE_DB_GO_MARKER", go.display().to_string()),
                ("RUNE_DB_OPENED_MARKER", opened.display().to_string()),
            ],
        ));
    }

    wait_ready_or_child_death(&mut children, &readies, MARKER_SAFETY_DEADLINE);
    touch(&go);

    for child in children {
        let output = child.wait_with_output().expect("wait child");
        assert!(
            output.status.success(),
            "race_open child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let verify = Connection::open(&path).expect("open verify connection");
    let integrity: String = verify
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .expect("integrity check");
    assert_eq!(integrity, "ok");
    let sessions: i64 = verify
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .expect("count sessions");
    assert_eq!(
        sessions, 2,
        "both racing opens must each get their own session row"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------
// Scenario (c): child SIGKILLed mid-storm
// ---------------------------------------------------------------------

#[test]
fn child_sigkilled_mid_storm_recovers_at_last_committed_batch_and_reaper_reclaims() {
    let dir = temp_dir("kill-mid-storm");
    let path = dir.join("rune-v1.db");
    let doc_ids = seed_schema_and_docs(&path, 1);
    let doc_id = doc_ids[0];
    let count = 200usize;
    let checkpoint = 50usize;

    let session_marker = dir.join("session");
    let checkpoint_marker = dir.join("checkpoint");
    let release_marker = dir.join("release"); // intentionally never written

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

    let killed_session_id: i64 = std::fs::read_to_string(&session_marker)
        .expect("read session marker")
        .trim()
        .parse()
        .expect("parse session id");

    // Parent "reopens": a fresh Store::open against the same path.
    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let (store, warning) =
        Store::open(&path, Arc::new(rune_vfs::Disk), on_event).expect("reopen store");
    assert!(warning.is_none());
    assert!(!store.degraded());

    // recover_document is scoped to a SESSION's own current_seq (never
    // touched by plain append_edit, so it defaults to "at head") — calling
    // it with the KILLED session's own id replays exactly its own committed
    // events, which is exactly the content the child had journaled before
    // being killed.
    let mut verify = Connection::open(&path).expect("verify connection");
    let recovered = {
        let tx = verify.transaction().expect("tx");
        let content =
            rune_db::recover_document(&tx, killed_session_id, doc_id).expect("recover_document");
        tx.commit().expect("commit");
        content
    };
    // Each edit inserts at position 0 (`start: 0, end: 0`), so content
    // accumulates with the LATEST insert first — the committed prefix reads
    // in descending order.
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

    // A new session appends past the dead one, establishing itself as the
    // new most-recent toucher of doc_id.
    let edit = AppliedEdit {
        start: recovered.len(),
        end: recovered.len(),
        deleted: String::new(),
        insert: "NEW".to_string(),
    };
    let id = store
        .append_edit(doc_id, &[edit], &[], &[])
        .expect("enqueue append");
    match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok { id: got, .. }) if got == id => {}
        other => panic!("expected append ack, got {other:?}"),
    }
    store.shutdown();

    // The killed child's real pid genuinely no longer exists, so the REAL
    // `is_process_alive` naturally reports it dead — no test override
    // needed. The reaper only reclaims a dead session once it is no longer
    // the most-recent toucher, which the append above just ensured.
    let mut reap_conn = Connection::open(&path).expect("reap connection");
    rune_db::reap_dead_sessions(&mut reap_conn, &rune_db::is_process_alive).expect("reap");

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
        killed_session_row_exists,
        "the sessions row itself must never be deleted"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------
// Scenario (d): two stores closing simultaneously
// ---------------------------------------------------------------------

#[test]
fn two_stores_closing_simultaneously_surface_no_error_despite_truncate_contention() {
    let dir = temp_dir("race-close");
    let path = dir.join("rune-v1.db");
    let _ = seed_schema_and_docs(&path, 0);
    let go = dir.join("go");

    let mut children = Vec::new();
    let mut readies = Vec::new();
    for i in 0..2 {
        let ready = dir.join(format!("ready-{i}"));
        readies.push(ready.clone());
        children.push(spawn_helper(
            "race_close",
            &[
                ("RUNE_DB_PATH", path.display().to_string()),
                ("RUNE_DB_READY_MARKER", ready.display().to_string()),
                ("RUNE_DB_GO_MARKER", go.display().to_string()),
            ],
        ));
    }

    wait_ready_or_child_death(&mut children, &readies, MARKER_SAFETY_DEADLINE);
    touch(&go);

    for child in children {
        let output = child.wait_with_output().expect("wait child");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "race_close child failed: {stderr}");
        assert!(
            !stderr.contains("panicked"),
            "child stderr shows a panic despite BUSY-class TRUNCATE contention being \
             expected and swallowed by design: {stderr}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------
// Scenario (f): reopen-after-external-atomic-swap data-loss regression
// ---------------------------------------------------------------------

#[test]
fn reopen_after_external_atomic_swap_bridges_the_dead_sessions_own_draft() {
    use rune_vfs::Vfs;

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

    // An external atomic-swap overwrite of the REAL file — a genuinely new
    // inode, exactly like another editor/tool replacing the file in place.
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

    let verify = Connection::open(&path).expect("open verify connection");
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

// ---------------------------------------------------------------------
// Scenario (e): sweep_unreferenced_blobs under real cross-process
// contention ([rune-db 8])
// ---------------------------------------------------------------------

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

    // A concurrent sweep must never delete a blob a surviving
    // snapshot/observation row still references — every reference must
    // resolve.
    let verify = Connection::open(&path).expect("open verify connection");
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

// ---------------------------------------------------------------------
// Scenario (j): the liveness-aware marker wait fails fast on a dead child
// ---------------------------------------------------------------------

#[test]
fn wait_ready_or_child_death_panics_immediately_when_a_child_dies_before_its_marker() {
    let dir = temp_dir("fail-fast");
    let marker = dir.join("never-touched");

    let mut children = vec![spawn_helper("defunct", &[])];
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_ready_or_child_death(
            &mut children,
            std::slice::from_ref(&marker),
            MARKER_SAFETY_DEADLINE,
        );
    }));

    let payload = result.expect_err("an unknown role must exit without ever touching its marker");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .expect("panic payload must be a string message");

    assert!(
        message.contains("child exited"),
        "a dead child must be reported by its own distinct message: {message}"
    );
    assert!(
        !message.contains("timed out"),
        "a dead child must never be reported as a timeout: {message}"
    );
    assert!(
        message.contains("unknown role defunct"),
        "the panic message must carry the dead child's captured stderr: {message}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
