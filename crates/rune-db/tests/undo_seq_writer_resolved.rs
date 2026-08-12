//! Regression for the writer-side `MoveUndoPos` resolution: through the
//! PUBLIC `Store` API alone (no `rune-tui` involved), an undo enqueued while
//! its own prior `AppendEdit`s are still unacknowledged, followed by a new
//! append, must never desynchronize the durable journal from what the
//! caller's own buffer actually holds — `recover_document`'s replay must
//! equal the caller's own bytes exactly.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use rune_core::buffer::AppliedEdit;
use rune_db::{DbEvent, OnEvent, OpOutcome, Store};
use rune_vfs::Disk;

fn temp_db_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rune-db-undo-seq-writer-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn recv(rx: &mpsc::Receiver<DbEvent>, id: u64) -> OpOutcome {
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(DbEvent::Ok { id: got, result }) if got == id => result,
        Ok(other) => panic!("op {id}: expected Ok, got {other:?}"),
        Err(e) => panic!("op {id}: timed out waiting for ack: {e}"),
    }
}

/// Appends `text` to `buf` and returns the `AppliedEdit` that inserts it at
/// the position it was appended — mirrors a real editor typing at the end
/// of the document.
fn append_and_edit(buf: &mut String, text: &str) -> AppliedEdit {
    let start = buf.len();
    buf.push_str(text);
    AppliedEdit {
        start,
        end: start,
        deleted: String::new(),
        insert: text.to_string(),
    }
}

#[test]
fn undo_then_append_with_in_flight_style_interleaving_matches_the_buffer() {
    let dir = temp_db_dir("basic");
    let db_path = dir.join("rune-v1.db");

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let (store, warning) = Store::open(&db_path, Arc::new(Disk), on_event).expect("open store");
    assert!(
        warning.is_none(),
        "must not degrade against a real temp path"
    );
    let session_id = store.session_id();

    let scratch_op = store.create_scratch().expect("enqueue create_scratch");
    let doc_id = match recv(&rx, scratch_op) {
        OpOutcome::RowId(id) => rune_db::DocId(id),
        other => panic!("expected RowId from CreateScratch, got {other:?}"),
    };

    let mut buf = String::new();

    // "alpha" — local position 1 — fully acked before anything else
    // happens.
    let edit = append_and_edit(&mut buf, "alpha");
    let op1 = store
        .append_edit(doc_id, &[edit], &[], &[])
        .expect("enqueue append alpha");
    assert!(matches!(recv(&rx, op1), OpOutcome::Seq(_)));

    // "beta" (local position 2) and "gamma" (local position 3) — enqueued
    // but their acks deliberately left UNDRAINED, simulating a typing burst
    // whose acks haven't round-tripped yet.
    let edit = append_and_edit(&mut buf, "beta");
    let op2 = store
        .append_edit(doc_id, &[edit], &[], &[])
        .expect("enqueue append beta");
    let edit = append_and_edit(&mut buf, "gamma");
    let op3 = store
        .append_edit(doc_id, &[edit], &[], &[])
        .expect("enqueue append gamma");

    // Undo "gamma" only: local position 2 — enqueued while op2/op3's own
    // acks are still in flight. The writer thread has already EXECUTED
    // both (strict FIFO single connection), so it resolves this exactly,
    // with no dependency on this caller having drained anything yet.
    let op4 = store
        .move_undo_pos(doc_id, 2)
        .expect("enqueue move_undo_pos");
    buf.truncate(buf.len() - "gamma".len());

    // A new append right after the undo — the exact step that corrupted the
    // durable journal before this fix, if `MoveUndoPos` had resolved to an
    // underestimate: `journal_append::append_edit`'s own truncation would
    // have deleted events this session never actually undid.
    let edit = append_and_edit(&mut buf, "delta");
    let op5 = store
        .append_edit(doc_id, &[edit], &[], &[])
        .expect("enqueue append delta");

    // Drain everything queued after "alpha", in FIFO arrival order —
    // asserting every single one is `Ok`, never `Err` (a truncated/
    // corrupted journal surfaces exactly there).
    for op_id in [op2, op3, op4, op5] {
        match recv(&rx, op_id) {
            OpOutcome::Seq(_) | OpOutcome::None => {}
            other => panic!("op {op_id}: unexpected outcome {other:?}"),
        }
    }

    store.shutdown();

    // A fresh, plain connection to the SAME db file — the definitive proof
    // the durable journal matches the buffer exactly, not just that no op
    // came back `Err`.
    let conn = rusqlite::Connection::open(&db_path).expect("open db file directly");
    let recovered = rune_db::recover_document(&conn, session_id, doc_id).expect("recover_document");
    assert_eq!(
        recovered, buf,
        "recover_document must equal the caller's own buffer bytes exactly"
    );
}
