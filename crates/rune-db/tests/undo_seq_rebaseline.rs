//! The writer's local-undo-position contract, through the PUBLIC `Store`
//! API alone, now that a caller's own [`rune_db::BindingToken`] is the
//! numbering key: the SAME token keeps its numbering across however many
//! `Load`s the caller enqueues against it (a reload, a re-baseline), so a
//! deep undo issued afterwards still resolves to its own durable seq — while
//! a caller minting a FRESH token for a binding starts that token's
//! numbering over, and a position only the OLD token ever ran is refused
//! outright rather than silently answered with some other token's seq.
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
use rune_db::{BindingToken, DbEvent, DocId, OnEvent, OpOutcome, Seq, Store};
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
/// appending at the caret does, under `token`'s own numbering.
fn type_at_end(
    store: &Store,
    rx: &mpsc::Receiver<DbEvent>,
    doc_id: DocId,
    token: BindingToken,
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
        .append_edit(doc_id, token, Seq(0), &[edit], &[], &[])
        .expect("enqueue append");
    assert!(matches!(ok(rx, op), OpOutcome::Seq(_)));
}

fn recovered_at(db_path: &Path, session_id: rune_db::SessionId, doc_id: DocId) -> String {
    let conn =
        rune_db::open_raw_connection_at_path_for_test(db_path).expect("open db file directly");
    rune_db::recover_document(&conn, session_id, doc_id)
        .expect("recover_document")
        .content
}

/// A caller reloading the SAME row under the SAME `BindingToken` keeps that
/// token's numbering, so an undo reaching PAST the reload still names its
/// own durable seq — the caller's deep undo positions survive a save's
/// lost-bookkeeping reload instead of degrading into forward replace-all
/// re-bases.
#[test]
fn the_same_token_keeps_deep_undo_positions_resolvable_across_a_reload() {
    let dir = temp_dir("same-row");
    let db_path = dir.join("rune-v1.db");
    let doc_path = seed_file(&dir, "a.md", "seed");
    let (store, rx) = open_store(&db_path);
    let session_id = store.session_id();

    let doc_id = bind(&store, &rx, &doc_path);
    let token = BindingToken::next();
    let mut buf = String::from("seed");
    type_at_end(&store, &rx, doc_id, token, &mut buf, "alpha");
    type_at_end(&store, &rx, doc_id, token, &mut buf, "beta");
    type_at_end(&store, &rx, doc_id, token, &mut buf, "gamma");

    let reload = store.load(&doc_path).expect("enqueue reload");
    match ok(&rx, reload) {
        OpOutcome::Load(result) => assert_eq!(
            result.doc_id, doc_id,
            "the reload must resolve the very row it named"
        ),
        other => panic!("expected a Load ack, got {other:?}"),
    }

    // Local position 1 — "alpha" only, two entries deeper than the reload.
    // A restarted numbering (a fresh token) has no entry there at all.
    let undo = store
        .move_undo_pos(doc_id, token, Seq(0), 1)
        .expect("enqueue move_undo_pos");
    assert!(
        matches!(ok(&rx, undo), OpOutcome::None),
        "a preserved token's numbering must resolve a pre-reload position"
    );
    buf.truncate("seedalpha".len());

    // The append after the undo truncates the abandoned future — the step
    // that turns a mis-resolved position into lost or resurrected text.
    type_at_end(&store, &rx, doc_id, token, &mut buf, "delta");
    store.shutdown();

    assert_eq!(
        recovered_at(&db_path, session_id, doc_id),
        buf,
        "recovery must reconstruct exactly the undone-to buffer"
    );
    assert_eq!(buf, "seedalphadelta");
}

/// A caller minting a FRESH `BindingToken` for a binding it already had —
/// exactly what a rebind that restarts numbering does — starts that token's
/// numbering over: the old token's positions no longer exist under the new
/// one and are refused outright rather than silently answered with some
/// other token's seq.
#[test]
fn a_fresh_token_refuses_a_position_only_the_old_token_ever_ran() {
    let dir = temp_dir("cross-row");
    let db_path = dir.join("rune-v1.db");
    let path_b = seed_file(&dir, "b.md", "bbb");
    let (store, rx) = open_store(&db_path);

    let doc_b = bind(&store, &rx, &path_b);
    let old_token = BindingToken::next();
    let mut buf_b = String::from("bbb");
    type_at_end(&store, &rx, doc_b, old_token, &mut buf_b, "one");

    // A restart mints a brand-new token for the SAME row.
    let fresh_token = BindingToken::next();
    let undo = store
        .move_undo_pos(doc_b, fresh_token, Seq(0), 1)
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
/// their durable seqs, so a numbering keyed by document alone would resolve
/// an undo one entry off into the OTHER document's seq. Keyed by
/// `BindingToken` instead, every undo still resolves inside its own token's
/// own lineage regardless of interleaving — proved by both documents'
/// recovery, since a wrong seq truncates the wrong tail.
#[test]
fn an_undo_never_resolves_into_another_documents_lineage() {
    let dir = temp_dir("lineage");
    let db_path = dir.join("rune-v1.db");
    let path_a = seed_file(&dir, "a.md", "A:");
    let path_b = seed_file(&dir, "b.md", "B:");
    let (store, rx) = open_store(&db_path);
    let session_id = store.session_id();

    let doc_a = bind(&store, &rx, &path_a);
    let doc_b = bind(&store, &rx, &path_b);
    let token_a = BindingToken::next();
    let token_b = BindingToken::next();
    let mut buf_a = String::from("A:");
    let mut buf_b = String::from("B:");

    // Interleaved, so no two consecutive seqs belong to the same document:
    // an off-by-one resolution can only land on the other lineage.
    type_at_end(&store, &rx, doc_a, token_a, &mut buf_a, "1");
    type_at_end(&store, &rx, doc_b, token_b, &mut buf_b, "1");
    type_at_end(&store, &rx, doc_a, token_a, &mut buf_a, "2");
    type_at_end(&store, &rx, doc_b, token_b, &mut buf_b, "2");
    type_at_end(&store, &rx, doc_a, token_a, &mut buf_a, "3");
    type_at_end(&store, &rx, doc_b, token_b, &mut buf_b, "3");

    let reload = store.load(&path_a).expect("enqueue reload");
    assert!(matches!(ok(&rx, reload), OpOutcome::Load(_)));

    let undo = store
        .move_undo_pos(doc_a, token_a, Seq(0), 1)
        .expect("enqueue move_undo_pos");
    assert!(matches!(ok(&rx, undo), OpOutcome::None));
    buf_a.truncate("A:1".len());
    type_at_end(&store, &rx, doc_a, token_a, &mut buf_a, "4");
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
