//! Tests for the writer thread's queue/dispatch loop — split out to keep
//! the parent under the file-size ceiling, the same shape
//! `decode_cmd_tests.rs` (rune-tui) already uses elsewhere in the workspace.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::*;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rune_core::buffer::AppliedEdit;

fn open_ready_connection() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory connection");
    crate::schema::apply(&conn).expect("apply schema");
    conn
}

fn test_vfs() -> Arc<dyn Vfs + Send + Sync> {
    Arc::new(rune_vfs::Mem::new())
}

#[test]
fn noop_op_round_trips_ok() {
    let events: Arc<Mutex<Vec<DbEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let events_for_cb = Arc::clone(&events);
    let on_event: OnEvent = Box::new(move |evt| {
        events_for_cb
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(evt);
    });

    let handle = spawn(open_ready_connection(), test_vfs(), on_event);
    handle
        .try_send(WriteOp {
            id: 7,
            kind: OpKind::Noop,
        })
        .expect("enqueue noop");

    // Dropping the sender (inside shutdown) closes the queue once
    // drained, so `join` returns only after the writer thread's `recv`
    // loop has processed our op and exited — deterministic, no polling.
    // `shutdown` also enqueues its own `OpKind::Shutdown` housekeeping
    // op (WP6.S2), which posts a second `DbEvent` for id 0 — assert on
    // the noop's own id rather than the events vec's exact length.
    handle.shutdown(SessionId(1), Arc::new(|_pid, _started_at| false));

    let events = events.lock().unwrap_or_else(|p| p.into_inner());
    assert!(
        events.iter().any(|e| matches!(
            e,
            DbEvent::Ok {
                id: 7,
                result: OpOutcome::None
            }
        )),
        "expected an Ok(id: 7, result: None) among {events:?}"
    );
}

/// Proves `OpKind::AppendEdit` runs end-to-end through the writer
/// thread's `BEGIN IMMEDIATE`/retry chokepoint and echoes the inserted
/// journal seq back via `DbEvent::Ok.result` (plan Hard rule: "every
/// write op flows through the WP2 writer FIFO/BEGIN IMMEDIATE
/// machinery"). Domain correctness (coalescing, replay) is covered at
/// the connection level in `journal.rs`/`tests/replay_equivalence.rs` —
/// this test only exercises the async plumbing.
#[test]
fn append_edit_op_runs_through_the_writer_and_echoes_seq() {
    let conn = open_ready_connection();
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
        [],
    )
    .expect("seed document");
    let doc_id = DocId(conn.last_insert_rowid());
    let session_id = crate::session::establish_session(&conn, SystemTime::now()).expect("session");

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let handle = spawn(conn, test_vfs(), on_event);

    handle
        .try_send(WriteOp {
            id: 1,
            kind: OpKind::AppendEdit {
                session_id,
                now: SystemTime::now(),
                doc_id,
                edits: vec![AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: "hi".to_string(),
                }],
                cursors_before: vec![],
                cursors_after: vec![],
            },
        })
        .expect("enqueue AppendEdit");

    let evt = rx.recv().expect("append edit completion");
    match evt {
        DbEvent::Ok { id: 1, result } => {
            assert_eq!(
                result,
                OpOutcome::Seq(Seq(1)),
                "first event for this doc must be seq 1"
            );
        }
        other => panic!("expected Ok(id:1, result:Seq(seq)), got {other:?}"),
    }

    handle.shutdown(session_id, Arc::new(crate::session::is_process_alive));
}

#[test]
fn stalled_writer_returns_full_without_blocking_or_panicking() {
    let (block_tx, block_rx) = mpsc::channel::<()>();
    let on_event: OnEvent = Box::new(|_evt| {});

    let handle = spawn(open_ready_connection(), test_vfs(), on_event);

    // The first op stalls the writer thread indefinitely until we
    // signal it — a deterministic rendezvous, not a sleep.
    handle
        .try_send(WriteOp {
            id: 0,
            kind: OpKind::TestBlock(block_rx),
        })
        .expect("enqueue the stalling op");

    let mut saw_full = false;
    for i in 1..=(QUEUE_DEPTH as u64 + 8) {
        match handle.try_send(WriteOp {
            id: i,
            kind: OpKind::Noop,
        }) {
            Ok(()) => {}
            Err(Error::WriterQueueFull) => {
                saw_full = true;
                break;
            }
            Err(other) => panic!("unexpected error enqueueing: {other}"),
        }
    }
    assert!(
        saw_full,
        "a stalled writer with a bounded queue must eventually return Full"
    );

    // Unblock the writer so it can drain the rest of the queue and the
    // thread can exit cleanly during shutdown.
    let _ = block_tx.send(());
    drop(block_tx);
    handle.shutdown(SessionId(1), Arc::new(|_pid, _started_at| false));
}
