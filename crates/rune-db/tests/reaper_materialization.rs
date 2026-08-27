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
use rune_db::{BindingToken, DbEvent, DocId, EditBatch, OnEvent, OpOutcome, Seq, Store};
use rune_vfs::Disk;

fn temp_db_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rune-db-reaper-materialization-{label}-{}-{}",
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

fn load(store: &Store, rx: &mpsc::Receiver<DbEvent>, path: &std::path::Path) -> DocId {
    let op = store.load(path).expect("enqueue load");
    match recv(rx, op) {
        DbEvent::Ok {
            result: OpOutcome::Load(result),
            ..
        } => result.doc_id,
        other => panic!("expected Load outcome, got {other:?}"),
    }
}

fn append(store: &Store, rx: &mpsc::Receiver<DbEvent>, doc_id: DocId, insert: &str) {
    let edit = AppliedEdit {
        start: 0,
        end: 0,
        deleted: String::new(),
        insert: insert.to_string(),
    };
    let op = store
        .append_edit(
            doc_id,
            BindingToken::next(),
            Seq(0),
            EditBatch {
                edits: &[edit],
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("enqueue append_edit");
    match recv(rx, op) {
        DbEvent::Ok { .. } => {}
        other => panic!("expected append_edit to ack, got {other:?}"),
    }
}

const SYNTHETIC_DEAD_PID: i64 = 111_111;

fn mark_session_dead(db_path: &std::path::Path, session_id: rune_db::SessionId) {
    let conn = rune_db::open_raw_connection_at_path_for_test(db_path).expect("open raw connection");
    conn.execute(
        "UPDATE sessions SET pid=?1, proc_started_at='synthetic-dead' WHERE id=?2",
        rusqlite::params![SYNTHETIC_DEAD_PID, session_id.0],
    )
    .expect("mark session dead");
}

#[test]
fn a_crashed_windows_unsaved_edits_survive_being_superseded_by_a_second_window() {
    let dir = temp_db_dir("two-window-crash");
    let db_path = dir.join("rune-v1.db");
    let doc_path = dir.join("notes.md");
    std::fs::write(&doc_path, "hello\n").expect("seed doc file");

    let (store_a, rx_a) = open_store(&db_path);
    let session_a = store_a.session_id();
    let doc_id = load(&store_a, &rx_a, &doc_path);
    append(&store_a, &rx_a, doc_id, "A's unsaved edit ");
    store_a.shutdown();

    mark_session_dead(&db_path, session_a);

    let (store_b, rx_b) = open_store(&db_path);
    let doc_id_b = load(&store_b, &rx_b, &doc_path);
    assert_eq!(
        doc_id_b, doc_id,
        "both windows must resolve the same document"
    );
    append(&store_b, &rx_b, doc_id, "B's later edit ");
    store_b.shutdown();

    let mut reap_conn =
        rune_db::open_recovery_store_at_path_for_test(&db_path).expect("open recovery store");
    rune_db::reap_dead_sessions(&mut reap_conn, &|pid, _| pid != SYNTHETIC_DEAD_PID, None)
        .expect("reap");

    let a_events: i64 = reap_conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE session_id=?1",
            [session_a.0],
            |r| r.get(0),
        )
        .expect("count a's events");
    assert_eq!(
        a_events, 1,
        "the crashed window's unsaved edit must never be reaped just because a later \
         window has since edited the same document"
    );

    let recovered = {
        let tx = reap_conn.transaction().expect("tx");
        let content = rune_db::recover_document(&tx, session_a, doc_id)
            .expect("recover_document")
            .content;
        tx.commit().expect("commit");
        content
    };
    assert_eq!(
        recovered, "A's unsaved edit hello\n",
        "the crashed window's own draft must still reconstruct after the reap"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
