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
use rune_core::undo::EditKind;
use rune_db::{DbEvent, EditBatch, OnEvent, Store};

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
    let doc_id = rune_db::DocId(env_var("RUNE_DB_DOC_ID").parse().expect("doc id"));
    let count: usize = env_var("RUNE_DB_COUNT").parse().expect("count");
    let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
    let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let store = open_store(&path, on_event);
    let token = rune_db::BindingToken::next();

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
            .append_edit(
                doc_id,
                token,
                rune_db::Seq(0),
                EditBatch {
                    edits: &[edit],
                    cursors_before: &[],
                    cursors_after: &[],
                    kind: EditKind::Other,
                },
            )
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
    let doc_id = rune_db::DocId(env_var("RUNE_DB_DOC_ID").parse().expect("doc id"));
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
    let token = rune_db::BindingToken::next();

    for i in 0..count {
        let edit = AppliedEdit {
            start: 0,
            end: 0,
            deleted: String::new(),
            insert: format!("{i} "),
        };
        let id = store
            .append_edit(
                doc_id,
                token,
                rune_db::Seq(0),
                EditBatch {
                    edits: &[edit],
                    cursors_before: &[],
                    cursors_after: &[],
                    kind: EditKind::Other,
                },
            )
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

fn recv_seq(rx: &mpsc::Receiver<DbEvent>, id: u64) -> rune_db::Seq {
    match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::Seq(seq),
        }) if got == id => seq,
        Ok(other) => panic!("expected Ok(id:{id}, Seq(_)), got {other:?}"),
        Err(e) => panic!("timed out waiting for ack of op {id}: {e}"),
    }
}

#[path = "helper_lifecycle.rs"]
mod helper_lifecycle;
pub(crate) use helper_lifecycle::{
    edit_and_die, gc_editor, gc_sweeper, reload_diverged, save_and_die,
};
