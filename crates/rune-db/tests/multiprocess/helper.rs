//! The child-process roles `helper_entrypoint` dispatches to — one per
//! multiprocess scenario. Every role opens its own
//! `Store` against the shared path a scenario test hands it via
//! `RUNE_DB_PATH`, synchronizes with its siblings through the ready/go
//! marker handshake `support` provides, then calls `std::process::exit`
//! itself on success.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use rune_core::buffer::AppliedEdit;
use rune_db::{DbEvent, OnEvent, Store};

use crate::support::{MARKER_SAFETY_DEADLINE, touch, wait_for_path};

fn env_var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("missing required env var {name}"))
}

fn db_path() -> PathBuf {
    PathBuf::from(env_var("RUNE_DB_PATH"))
}

fn open_store(path: &Path, on_event: OnEvent) -> Store {
    let (store, warning) =
        Store::open(path, Arc::new(rune_vfs::Disk), on_event).expect("child: open store");
    assert!(
        warning.is_none(),
        "child: must not degrade against a real writable temp path"
    );
    assert!(!store.degraded());
    store
}

fn expect_ok(rx: &mpsc::Receiver<DbEvent>, id: u64) {
    match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok { id: got, .. }) if got == id => {}
        Ok(other) => panic!("expected Ok(id:{id}), got {other:?}"),
        Err(e) => panic!("timed out waiting for ack of op {id}: {e}"),
    }
}

/// Role (a): append `RUNE_DB_COUNT` edits to `RUNE_DB_DOC_ID`, waiting
/// for each ack before sending the next (read-your-writes, one writer
/// at a time from THIS process's own perspective — the other sibling
/// children racing the SAME db file is exactly what the scenario is
/// testing). Synchronizes its start with its siblings via a
/// ready/go marker handshake so the storm genuinely overlaps in time.
pub(crate) fn append_storm() {
    let path = db_path();
    let doc_id: i64 = env_var("RUNE_DB_DOC_ID").parse().expect("doc id");
    let count: usize = env_var("RUNE_DB_COUNT").parse().expect("count");
    let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
    let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let store = open_store(&path, on_event);

    touch(&ready);
    wait_for_path(&go, MARKER_SAFETY_DEADLINE);

    for i in 0..count {
        let edit = AppliedEdit {
            start: 0,
            end: 0,
            deleted: String::new(),
            insert: format!("{i} "),
        };
        let id = store
            .append_edit(doc_id, &[edit], &[], &[])
            .expect("enqueue append");
        expect_ok(&rx, id);
    }

    store.shutdown();
    std::process::exit(0);
}

/// Role (c): like `append_storm`, but after the `RUNE_DB_CHECKPOINT`-th
/// committed append it writes its own session id and a checkpoint
/// marker, then BLOCKS waiting for a release marker the parent never
/// writes — the parent SIGKILLs this process while it is blocked here,
/// giving a deterministic, race-free "killed after exactly N committed
/// batches" instant (no window between "read progress" and "issue
/// kill").
pub(crate) fn append_storm_checkpoint() {
    let path = db_path();
    let doc_id: i64 = env_var("RUNE_DB_DOC_ID").parse().expect("doc id");
    let count: usize = env_var("RUNE_DB_COUNT").parse().expect("count");
    let checkpoint: usize = env_var("RUNE_DB_CHECKPOINT").parse().expect("checkpoint");
    let session_marker = PathBuf::from(env_var("RUNE_DB_SESSION_MARKER"));
    let checkpoint_marker = PathBuf::from(env_var("RUNE_DB_CHECKPOINT_MARKER"));
    let release_marker = PathBuf::from(env_var("RUNE_DB_RELEASE_MARKER"));

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let store = open_store(&path, on_event);
    std::fs::write(&session_marker, store.session_id().to_string()).expect("write session marker");

    for i in 0..count {
        let edit = AppliedEdit {
            start: 0,
            end: 0,
            deleted: String::new(),
            insert: format!("{i} "),
        };
        let id = store
            .append_edit(doc_id, &[edit], &[], &[])
            .expect("enqueue append");
        expect_ok(&rx, id);

        if i + 1 == checkpoint {
            touch(&checkpoint_marker);
            // Safety-net deadline only: the scenario that spawns this
            // role always kills the process long before this elapses.
            wait_for_path(&release_marker, MARKER_SAFETY_DEADLINE);
        }
    }

    store.shutdown();
    std::process::exit(0);
}

/// Role (b): race `Store::open` itself against a fresh (not yet
/// existing) path, synchronized with the sibling via ready/go markers.
pub(crate) fn race_open() {
    let path = db_path();
    let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
    let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));
    let opened_marker = PathBuf::from(env_var("RUNE_DB_OPENED_MARKER"));

    touch(&ready);
    wait_for_path(&go, MARKER_SAFETY_DEADLINE);

    let store = open_store(&path, Box::new(|_evt| {}));
    std::fs::write(&opened_marker, store.session_id().to_string()).expect("write opened marker");
    store.shutdown();
    std::process::exit(0);
}

/// Role (d): open, then force this session to see every OTHER session
/// as dead (`Store::set_liveness_check`) so its own shutdown
/// unconditionally attempts a TRUNCATE checkpoint regardless of whether
/// the real sibling process has actually exited yet — synchronized with
/// the sibling so both call `shutdown` at nearly the same instant,
/// deterministically forcing a genuine TRUNCATE race between two real
/// OS processes.
pub(crate) fn race_close() {
    let path = db_path();
    let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
    let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));

    let store = open_store(&path, Box::new(|_evt| {}));
    store.set_liveness_check(Arc::new(|_pid, _started_at| false));

    touch(&ready);
    wait_for_path(&go, MARKER_SAFETY_DEADLINE);

    store.shutdown();
    std::process::exit(0);
}

fn recv_seq(rx: &mpsc::Receiver<DbEvent>, id: u64) -> i64 {
    match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::Seq(seq),
        }) if got == id => seq,
        Ok(other) => panic!("expected Ok(id:{id}, Seq(_)), got {other:?}"),
        Err(e) => panic!("timed out waiting for ack of op {id}: {e}"),
    }
}

/// Role (e): [rune-db 8]'s coverage gap — `sweep_unreferenced_blobs` had
/// never been exercised under real cross-process contention. This role
/// repeatedly orphans a blob via the SAME mechanism
/// `journal::new_edit_after_undo_truncates_the_abandoned_future` proves
/// in-process: snapshot a piece of content, undo back to the start,
/// then commit a DIVERGENT edit — the truncation deletes the snapshot
/// row anchored past the new position, orphaning the blob it referenced
/// (`snapshot.rs`'s module doc: journal truncation "deletes both
/// `events` and `snapshots` rows, but the blob a surviving snapshot
/// still points to is untouched" — an orphaned one is fair game for the
/// sibling `gc_sweeper` process racing this one). Every op's ack is
/// asserted `Ok` — the actual claim under test is that concurrent
/// sweeping from another process never causes a legitimate write here
/// to fail or corrupt.
pub(crate) fn gc_editor() {
    let path = db_path();
    let doc_id: i64 = env_var("RUNE_DB_DOC_ID").parse().expect("doc id");
    let count: usize = env_var("RUNE_DB_COUNT").parse().expect("count");
    let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
    let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let store = open_store(&path, on_event);

    touch(&ready);
    wait_for_path(&go, MARKER_SAFETY_DEADLINE);

    for i in 0..count {
        let content_a = format!("round-{i}-a");
        let insert_a = AppliedEdit {
            start: 0,
            end: 0,
            deleted: String::new(),
            insert: content_a.clone(),
        };
        let id = store
            .append_edit(doc_id, &[insert_a], &[], &[])
            .expect("enqueue append a");
        recv_seq(&rx, id);

        let id = store
            .create_snapshot(doc_id, &content_a)
            .expect("enqueue snapshot");
        expect_ok(&rx, id);

        let id = store
            .move_undo_pos(doc_id, 0)
            .expect("enqueue move_undo_pos");
        expect_ok(&rx, id);

        // Diverges from `content_a` — truncates the now-abandoned
        // future, including the snapshot just created, orphaning its
        // blob for the sibling sweeper to find.
        let insert_b = AppliedEdit {
            start: 0,
            end: 0,
            deleted: String::new(),
            insert: format!("round-{i}-b"),
        };
        let id = store
            .append_edit(doc_id, &[insert_b], &[], &[])
            .expect("enqueue append b");
        expect_ok(&rx, id);
    }

    store.shutdown();
    std::process::exit(0);
}

/// Role (g): opens `RUNE_DB_DOC_PATH` (a REAL file), journals one unsaved
/// edit on top of it, writes the resulting `doc_id` to
/// `RUNE_DB_DOC_ID_MARKER`, then exits WITHOUT calling `store.shutdown()` —
/// the abrupt, store-preserved quit (`^C^C`) the data-loss regression
/// starts from. The edit's own `append_edit` ack already committed
/// synchronously (WAL), so skipping shutdown loses nothing durable.
pub(crate) fn edit_and_die() {
    let path = db_path();
    let doc_path = PathBuf::from(env_var("RUNE_DB_DOC_PATH"));
    let doc_id_marker = PathBuf::from(env_var("RUNE_DB_DOC_ID_MARKER"));

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let store = open_store(&path, on_event);

    let id = store.load(&doc_path).expect("enqueue load");
    let doc_id = match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::Load(result),
        }) if got == id => result.doc_id,
        other => panic!("expected load ack, got {other:?}"),
    };

    let edit = AppliedEdit {
        start: 0,
        end: 0,
        deleted: String::new(),
        insert: "UNSAVED ".to_string(),
    };
    let id = store
        .append_edit(doc_id, &[edit], &[], &[])
        .expect("enqueue append");
    expect_ok(&rx, id);

    std::fs::write(&doc_id_marker, doc_id.to_string()).expect("write doc id marker");
    std::process::exit(0);
}

/// Role (h): reopens `RUNE_DB_DOC_PATH` via a FRESH `Store`/session (the
/// next process), writes the resulting `LoadResult::recovered` content and
/// `sync.kind` to their own marker files for the parent to assert on. The
/// data-loss regression's actual claim under test.
pub(crate) fn reload_diverged() {
    let path = db_path();
    let doc_path = PathBuf::from(env_var("RUNE_DB_DOC_PATH"));
    let recovered_marker = PathBuf::from(env_var("RUNE_DB_RECOVERED_MARKER"));
    let sync_marker = PathBuf::from(env_var("RUNE_DB_SYNC_MARKER"));

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let store = open_store(&path, on_event);

    let id = store.load(&doc_path).expect("enqueue load");
    let load_result = match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::Load(result),
        }) if got == id => *result,
        other => panic!("expected load ack, got {other:?}"),
    };

    std::fs::write(&recovered_marker, &load_result.recovered).expect("write recovered marker");
    std::fs::write(&sync_marker, format!("{:?}", load_result.sync.kind))
        .expect("write sync marker");

    store.shutdown();
    std::process::exit(0);
}

/// Role (f): the sibling of [`gc_editor`] — repeatedly opens and closes
/// its OWN `Store` against the same shared path. Every `Store::open`
/// runs a best-effort startup blob sweep (`store.rs`'s doc: "One
/// startup blob-sweep batch ... after the reaper"), so this role's
/// open/close loop is what actually generates the real cross-process
/// `sweep_unreferenced_blobs` contention against `gc_editor`'s
/// concurrent orphaning — the exact gap [rune-db 8] names.
pub(crate) fn gc_sweeper() {
    let path = db_path();
    let count: usize = env_var("RUNE_DB_COUNT").parse().expect("count");
    let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
    let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));

    touch(&ready);
    wait_for_path(&go, MARKER_SAFETY_DEADLINE);

    for _ in 0..count {
        let store = open_store(&path, Box::new(|_evt| {}));
        store.shutdown();
    }

    std::process::exit(0);
}
