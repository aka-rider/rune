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
use rune_core::undo::EditKind;
use rune_db::{DbEvent, EditBatch, OnEvent, OpOutcome, Store};
use rune_vfs::Disk;

fn temp_db_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rune-db-scratch-gc-liveness-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn open_store(db_path: &std::path::Path) -> (Store, mpsc::Receiver<DbEvent>) {
    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let (store, warning) = Store::open(db_path, Arc::new(Disk), on_event).expect("open store");
    assert!(
        warning.is_none(),
        "must not degrade against a real temp path"
    );
    (store, rx)
}

fn recv(rx: &mpsc::Receiver<DbEvent>, id: u64) -> DbEvent {
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|e| panic!("op {id}: timed out waiting for ack: {e}"))
}

fn create_scratch(store: &Store, rx: &mpsc::Receiver<DbEvent>) -> rune_db::DocId {
    let op = store.create_scratch().expect("enqueue create_scratch");
    match recv(rx, op) {
        DbEvent::Ok {
            result: OpOutcome::ScratchDocId(id),
            ..
        } => id,
        other => panic!("expected ScratchDocId from CreateScratch, got {other:?}"),
    }
}

#[test]
fn a_second_bare_launch_never_disables_recovery_for_the_first_sessions_fresh_draft() {
    let dir = temp_db_dir("basic");
    let db_path = dir.join("rune-v1.db");

    let (store_a, rx_a) = open_store(&db_path);
    let doc_a = create_scratch(&store_a, &rx_a);

    let (store_b, rx_b) = open_store(&db_path);
    let doc_b = create_scratch(&store_b, &rx_b);

    let gc_op = store_b
        .gc_empty_scratch(doc_b.0)
        .expect("enqueue gc_empty_scratch");
    match recv(&rx_b, gc_op) {
        DbEvent::Ok { .. } => {}
        other => panic!("expected Ok from GcEmptyScratch, got {other:?}"),
    }

    let raw =
        rune_db::open_raw_connection_at_path_for_test(&db_path).expect("open db file directly");
    let doc_a_present: bool = raw
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            [doc_a.0],
            |r| r.get(0),
        )
        .expect("check doc a's row");
    assert!(
        doc_a_present,
        "a second bare launch must never sweep away another, still-running \
         session's freshly minted scratch draft"
    );

    let edit = AppliedEdit {
        start: 0,
        end: 0,
        deleted: String::new(),
        insert: "hello".to_string(),
    };
    let append_op = store_a
        .append_edit(
            doc_a,
            rune_db::BindingToken::next(),
            rune_db::Seq(0),
            EditBatch {
                edits: &[edit],
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("enqueue append_edit");
    match recv(&rx_a, append_op) {
        DbEvent::Ok { .. } => {}
        DbEvent::Err { error, .. } => {
            panic!("the first session's own draft must still accept edits: {error}")
        }
        other => panic!("unexpected completion for append_edit: {other:?}"),
    }

    store_a.shutdown();
    store_b.shutdown();
}
