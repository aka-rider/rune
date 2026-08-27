#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;
use crate::tests::ScratchHome;
use rune_vfs::Mem;

fn open_store_at(home: &Path) -> (Arc<DbBridge>, Store) {
    let db_path = db_path_for(Some(home)).expect("db path for a real home");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let bridge = DbBridge::bootstrap();
    let (store, _warning) = Store::open(&db_path, vfs, bridge.on_event()).expect("open store");
    (bridge, store)
}

fn await_ack(bridge: &DbBridge, op_id: u64) -> OpOutcome {
    match bridge
        .wait_for_bootstrap_event(|evt| matches!(evt, DbEvent::Ok { id, .. } if *id == op_id))
    {
        DbEvent::Ok { result, .. } => result,
        other => panic!("expected an Ok ack for op {op_id}, got {other:?}"),
    }
}

fn mark_every_session_dead(db_path: &Path) {
    let raw =
        rune_db::open_raw_connection_at_path_for_test(db_path).expect("open db file directly");
    raw.execute("UPDATE sessions SET pid = -1", [])
        .expect("mark every recorded session dead");
}

#[test]
fn db_path_for_rejects_an_empty_home() {
    assert!(
        db_path_for(Some(Path::new(""))).is_none(),
        "an empty $HOME must never be treated as a usable recovery-store home"
    );
}

#[test]
fn db_path_for_is_none_without_a_home() {
    assert!(db_path_for(None).is_none());
}

#[test]
fn from_string_for_db_bootstrap_untitled_carries_the_banner() {
    let result: DbBootstrapUntitled = "boom".to_string().into();
    assert_eq!(result.banner.as_deref(), Some("boom"));
}

#[test]
fn degrade_formats_the_banner() {
    let home = ScratchHome::new("degrade-banner");
    let (_bridge, store) = open_store_at(&home.0);

    let result = degrade(store, "boom");

    assert_eq!(result.banner.as_deref(), Some("recovery disabled: boom"));
}

#[test]
fn degrade_untitled_formats_the_banner() {
    let home = ScratchHome::new("degrade-untitled-banner");
    let (_bridge, store) = open_store_at(&home.0);

    let result = degrade_untitled(store, "boom");

    assert_eq!(result.banner.as_deref(), Some("recovery disabled: boom"));
}

#[test]
fn bootstrap_store_only_surfaces_the_degraded_banner() {
    let home = ScratchHome::new("store-only-degraded");
    let app_support = home.0.join("Library").join("Application Support");
    std::fs::create_dir_all(&app_support).expect("create Application Support");
    std::fs::write(app_support.join("rune"), b"occupying the directory slot")
        .expect("occupy the rune directory slot with a plain file");

    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let result = bootstrap_store_only(vfs, Some(&home.0));

    assert!(
        result.banner.is_some(),
        "a store that can only open through its in-memory fallback must still banner"
    );
}

#[test]
fn bootstrap_untitled_db_offers_back_a_whitespace_only_dead_session_draft() {
    let home = ScratchHome::new("untitled-whitespace");
    let db_path = db_path_for(Some(&home.0)).expect("db path for a real home");

    let old_db_id = {
        let (bridge, store) = open_store_at(&home.0);
        let create_op = store.create_scratch().expect("enqueue create_scratch");
        let doc_id = match await_ack(&bridge, create_op) {
            OpOutcome::ScratchDocId(id) => id.0,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };
        let snapshot_op = store
            .create_snapshot(rune_db::DocId(doc_id), "   ")
            .expect("enqueue create_snapshot");
        await_ack(&bridge, snapshot_op);
        store.shutdown();
        doc_id
    };

    mark_every_session_dead(&db_path);

    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let result = bootstrap_untitled_db(vfs, Some(&home.0));

    assert_eq!(
        result.scratch_docs.len(),
        1,
        "the whitespace-only draft is backed by a real snapshot and must be offered back, not orphaned"
    );
    assert_eq!(
        result.scratch_docs[0].db_id, old_db_id,
        "the crashed session's own row must be adopted, never a fresh one copying nothing in"
    );
    assert_eq!(
        result.scratch_docs[0].recovered.content, "   ",
        "the whitespace the user's session actually journaled must round-trip verbatim"
    );
}

#[test]
fn bootstrap_untitled_db_never_resurrects_a_row_with_no_journaled_history() {
    let home = ScratchHome::new("untitled-never-edited");
    let db_path = db_path_for(Some(&home.0)).expect("db path for a real home");

    let old_db_id = {
        let (bridge, store) = open_store_at(&home.0);
        let create_op = store.create_scratch().expect("enqueue create_scratch");
        let doc_id = match await_ack(&bridge, create_op) {
            OpOutcome::ScratchDocId(id) => id.0,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };
        store.shutdown();
        doc_id
    };

    mark_every_session_dead(&db_path);

    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let result = bootstrap_untitled_db(vfs, Some(&home.0));

    assert_eq!(
        result.scratch_docs.len(),
        1,
        "a row with no snapshot and no edit ever journaled behind it has nothing to recover"
    );
    assert_ne!(
        result.scratch_docs[0].db_id, old_db_id,
        "a never-edited scratch row must never resurrect as if it carried real content"
    );
}

#[test]
fn bootstrap_new_file_offers_back_a_whitespace_only_named_draft() {
    let home = ScratchHome::new("named-draft-whitespace");
    let db_path = db_path_for(Some(&home.0)).expect("db path for a real home");
    let intended_path = Path::new("/vault/notes.md");

    let old_db_id = {
        let (bridge, store) = open_store_at(&home.0);
        let create_op = store
            .create_named_scratch(&intended_path.to_string_lossy())
            .expect("enqueue create_named_scratch");
        let doc_id = match await_ack(&bridge, create_op) {
            OpOutcome::ScratchDocId(id) => id.0,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };
        let snapshot_op = store
            .create_snapshot(rune_db::DocId(doc_id), "   ")
            .expect("enqueue create_snapshot");
        await_ack(&bridge, snapshot_op);
        store.shutdown();
        doc_id
    };

    mark_every_session_dead(&db_path);

    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let result = bootstrap_new_file(vfs, intended_path, Some(&home.0));

    let doc_db = result.doc_db.expect("a scratch row must still be bound");
    assert_eq!(
        doc_db.db_id, old_db_id,
        "the crashed session's whitespace-only draft is backed by a real snapshot and must be inherited, never orphaned under a fresh row"
    );
    assert_eq!(
        result
            .recovered_content
            .expect("the journaled whitespace must be offered back as recovered_content")
            .content,
        "   "
    );
}

#[test]
fn bootstrap_new_file_never_resurrects_a_named_row_with_no_journaled_history() {
    let home = ScratchHome::new("named-draft-never-edited");
    let db_path = db_path_for(Some(&home.0)).expect("db path for a real home");
    let intended_path = Path::new("/vault/notes.md");

    let old_db_id = {
        let (bridge, store) = open_store_at(&home.0);
        let create_op = store
            .create_named_scratch(&intended_path.to_string_lossy())
            .expect("enqueue create_named_scratch");
        let doc_id = match await_ack(&bridge, create_op) {
            OpOutcome::ScratchDocId(id) => id.0,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };
        store.shutdown();
        doc_id
    };

    mark_every_session_dead(&db_path);

    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let result = bootstrap_new_file(vfs, intended_path, Some(&home.0));

    let doc_db = result.doc_db.expect("a scratch row must still be bound");
    assert_ne!(
        doc_db.db_id, old_db_id,
        "a never-edited named scratch row must never be inherited as if it carried real content"
    );
    assert!(
        result.recovered_content.is_none(),
        "a fresh named scratch row has no journaled content to offer back"
    );
}

/// `blocking_call`'s predicate must pick out its OWN op's ack, never a
/// different in-flight op's — the mutation this guards against
/// (`*id == op_id` flipped to `!=`) would make it grab whichever OTHER op's
/// ack shows up instead, and hang forever if none ever does (the `cargo
/// mutants` timeout this test replaces). Two scratch creates are enqueued
/// back-to-back so a wrong-id match has something to grab: `blocking_call`
/// is asked for `op1`'s ack while `op2`'s is still in flight, then whatever
/// is left in the bootstrap sink is pulled directly through the bridge
/// (bypassing `blocking_call` itself, so this second read can never be
/// fooled by the same mutation) to see which ack `blocking_call` actually
/// consumed.
#[test]
fn blocking_call_never_matches_a_different_operations_ack() {
    let home = ScratchHome::new("blocking-call-wrong-id");
    let (bridge, store) = open_store_at(&home.0);

    let op1 = store.create_scratch().expect("enqueue op1");
    let op2 = store.create_scratch().expect("enqueue op2");

    let claimed_for_op1 = blocking_call(&bridge, || Ok(op1)).expect("op1 should ack");
    let claimed_doc1 = match claimed_for_op1 {
        OpOutcome::ScratchDocId(id) => id.0,
        other => panic!("expected a CreateScratch ack, got {other:?}"),
    };

    let leftover = bridge.wait_for_bootstrap_event(|_evt| true);
    let (leftover_id, leftover_doc) = match leftover {
        DbEvent::Ok {
            id,
            result: OpOutcome::ScratchDocId(doc),
        } => (id, doc.0),
        other => panic!("expected a leftover CreateScratch ack, got {other:?}"),
    };

    assert_eq!(
        leftover_id, op2,
        "blocking_call(op1) must never consume op2's own ack"
    );
    assert_ne!(
        claimed_doc1, leftover_doc,
        "op1's and op2's own results must be the two distinct scratch rows created"
    );

    store.shutdown();
}
