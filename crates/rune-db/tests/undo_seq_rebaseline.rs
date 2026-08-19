//! The re-baseline half of the writer's local-undo-position contract,
//! through the PUBLIC `Store` API alone: `Store::load_rebaseline` naming the
//! row a caller is already bound to keeps that row's numbering, so a deep
//! undo issued afterwards still resolves to its own durable seq — while
//! every other load restarts the numbering, and a re-baseline that lands on
//! a DIFFERENT row than the one it named is one of those.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use rune_core::buffer::AppliedEdit;
use rune_db::{DbEvent, DocId, OnEvent, OpOutcome, Store};
use rune_vfs::Disk;

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rune-db-undo-rebaseline-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn recv(rx: &mpsc::Receiver<DbEvent>, id: u64) -> Result<OpOutcome, String> {
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(DbEvent::Ok { id: got, result }) if got == id => Ok(result),
        Ok(DbEvent::Err { id: got, error }) if got == id => Err(error),
        Ok(other) => panic!("op {id}: expected this op's ack, got {other:?}"),
        Err(e) => panic!("op {id}: timed out waiting for ack: {e}"),
    }
}

fn ok(rx: &mpsc::Receiver<DbEvent>, id: u64) -> OpOutcome {
    recv(rx, id).unwrap_or_else(|e| panic!("op {id} must succeed, got {e}"))
}

fn open_store(db_path: &Path) -> (Store, mpsc::Receiver<DbEvent>) {
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

fn seed_file(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).expect("seed file");
    path
}

fn bind(store: &Store, rx: &mpsc::Receiver<DbEvent>, path: &Path) -> DocId {
    let op = store.load(path).expect("enqueue load");
    match ok(rx, op) {
        OpOutcome::Load(result) => result.doc_id,
        other => panic!("expected a Load ack, got {other:?}"),
    }
}

/// Types `text` at the end of `buf` and journals it, exactly as an editor
/// appending at the caret does.
fn type_at_end(
    store: &Store,
    rx: &mpsc::Receiver<DbEvent>,
    doc_id: DocId,
    buf: &mut String,
    text: &str,
) {
    let start = buf.len();
    buf.push_str(text);
    let edit = AppliedEdit {
        start,
        end: start,
        deleted: String::new(),
        insert: text.to_string(),
    };
    let op = store
        .append_edit(doc_id, &[edit], &[], &[])
        .expect("enqueue append");
    assert!(matches!(ok(rx, op), OpOutcome::Seq(_)));
}

fn recovered_at(db_path: &Path, session_id: rune_db::SessionId, doc_id: DocId) -> String {
    let conn =
        rune_db::open_raw_connection_at_path_for_test(db_path).expect("open db file directly");
    rune_db::recover_document(&conn, session_id, doc_id).expect("recover_document")
}

/// A same-row re-baseline keeps the row's local-position numbering, so an
/// undo reaching PAST the re-baseline still names its own durable seq — the
/// caller's deep undo positions survive a save's lost-bookkeeping reload
/// instead of degrading into forward replace-all re-bases.
#[test]
fn a_same_row_rebaseline_keeps_deep_undo_positions_resolvable() {
    let dir = temp_dir("same-row");
    let db_path = dir.join("rune-v1.db");
    let doc_path = seed_file(&dir, "a.md", "seed");
    let (store, rx) = open_store(&db_path);
    let session_id = store.session_id();

    let doc_id = bind(&store, &rx, &doc_path);
    let mut buf = String::from("seed");
    type_at_end(&store, &rx, doc_id, &mut buf, "alpha");
    type_at_end(&store, &rx, doc_id, &mut buf, "beta");
    type_at_end(&store, &rx, doc_id, &mut buf, "gamma");

    let rebaseline = store
        .load_rebaseline(&doc_path, doc_id)
        .expect("enqueue re-baseline load");
    match ok(&rx, rebaseline) {
        OpOutcome::Load(result) => assert_eq!(
            result.doc_id, doc_id,
            "the re-baseline must resolve the very row it named"
        ),
        other => panic!("expected a Load ack, got {other:?}"),
    }

    // Local position 1 — "alpha" only, two entries deeper than the
    // re-baseline. A restarted numbering has no entry there at all.
    let undo = store
        .move_undo_pos(doc_id, 1)
        .expect("enqueue move_undo_pos");
    assert!(
        matches!(ok(&rx, undo), OpOutcome::None),
        "a preserved numbering must resolve a pre-re-baseline position"
    );
    buf.truncate("seedalpha".len());

    // The append after the undo truncates the abandoned future — the step
    // that turns a mis-resolved position into lost or resurrected text.
    type_at_end(&store, &rx, doc_id, &mut buf, "delta");
    store.shutdown();

    assert_eq!(
        recovered_at(&db_path, session_id, doc_id),
        buf,
        "recovery must reconstruct exactly the undone-to buffer"
    );
    assert_eq!(buf, "seedalphadelta");
}

/// A re-baseline that resolves to a DIFFERENT row than the one it named is
/// an ordinary bind for that row: its numbering restarts, so the caller's
/// old positions no longer exist and are refused outright rather than
/// silently answered with some other entry's seq.
#[test]
fn a_rebaseline_landing_on_another_row_restarts_that_rows_numbering() {
    let dir = temp_dir("cross-row");
    let db_path = dir.join("rune-v1.db");
    let path_a = seed_file(&dir, "a.md", "aaa");
    let path_b = seed_file(&dir, "b.md", "bbb");
    let (store, rx) = open_store(&db_path);

    let doc_a = bind(&store, &rx, &path_a);
    let doc_b = bind(&store, &rx, &path_b);
    let mut buf_b = String::from("bbb");
    type_at_end(&store, &rx, doc_b, &mut buf_b, "one");

    let crossed = store
        .load_rebaseline(&path_b, doc_a)
        .expect("enqueue re-baseline load");
    match ok(&rx, crossed) {
        OpOutcome::Load(result) => assert_eq!(result.doc_id, doc_b),
        other => panic!("expected a Load ack, got {other:?}"),
    }

    let undo = store
        .move_undo_pos(doc_b, 1)
        .expect("enqueue move_undo_pos");
    let refused =
        recv(&rx, undo).expect_err("a restarted numbering must refuse a position it never ran");
    assert!(
        refused.contains("local position 1"),
        "the refusal must name the unresolvable position, got {refused}"
    );
    store.shutdown();
}

/// The lineage hazard: two documents journaling into ONE session interleave
/// their durable seqs, so an undo resolved one entry off lands on the other
/// document's seq. Across a re-baseline of one of them, every undo must
/// still resolve inside its own document's lineage — proved by both
/// documents' recovery, since a wrong seq truncates the wrong tail.
#[test]
fn a_rebaselined_undo_never_resolves_into_another_documents_lineage() {
    let dir = temp_dir("lineage");
    let db_path = dir.join("rune-v1.db");
    let path_a = seed_file(&dir, "a.md", "A:");
    let path_b = seed_file(&dir, "b.md", "B:");
    let (store, rx) = open_store(&db_path);
    let session_id = store.session_id();

    let doc_a = bind(&store, &rx, &path_a);
    let doc_b = bind(&store, &rx, &path_b);
    let mut buf_a = String::from("A:");
    let mut buf_b = String::from("B:");

    // Interleaved, so no two consecutive seqs belong to the same document:
    // an off-by-one resolution can only land on the other lineage.
    type_at_end(&store, &rx, doc_a, &mut buf_a, "1");
    type_at_end(&store, &rx, doc_b, &mut buf_b, "1");
    type_at_end(&store, &rx, doc_a, &mut buf_a, "2");
    type_at_end(&store, &rx, doc_b, &mut buf_b, "2");
    type_at_end(&store, &rx, doc_a, &mut buf_a, "3");
    type_at_end(&store, &rx, doc_b, &mut buf_b, "3");

    let rebaseline = store
        .load_rebaseline(&path_a, doc_a)
        .expect("enqueue re-baseline load");
    assert!(matches!(ok(&rx, rebaseline), OpOutcome::Load(_)));

    let undo = store
        .move_undo_pos(doc_a, 1)
        .expect("enqueue move_undo_pos");
    assert!(matches!(ok(&rx, undo), OpOutcome::None));
    buf_a.truncate("A:1".len());
    type_at_end(&store, &rx, doc_a, &mut buf_a, "4");
    store.shutdown();

    assert_eq!(
        recovered_at(&db_path, session_id, doc_a),
        buf_a,
        "the re-baselined document must reconstruct its own undone-to buffer"
    );
    assert_eq!(
        recovered_at(&db_path, session_id, doc_b),
        buf_b,
        "the sibling document's journal must be untouched by the undo"
    );
}
