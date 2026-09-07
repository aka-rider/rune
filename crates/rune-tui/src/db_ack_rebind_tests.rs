#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::db::{Db, DbBridge};
use rune_core::buffer::Buffer;
use rune_db::{ClockFn, DbEvent, OpOutcome, Store};
use rune_vfs::{Mem, Vfs, VfsTestExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[test]
fn rebaseline_load_advances_expect_obs() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    vfs.save_atomic(Path::new("/doc.md"), b"hello")
        .expect("seed doc.md");
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = DbBridge::bootstrap();
    let store =
        Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("open store");

    store.load(Path::new("/doc.md")).expect("enqueue load");
    let load = match bridge.wait_for_bootstrap_event(|_| true) {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => *load,
        other => panic!("expected a Load ack, got {other:?}"),
    };

    let mut app = App::new(
        Buffer::new("hello"),
        Some(
            crate::resolved::ResolvedPath::resolve(
                vfs.as_ref(),
                std::path::Path::new(&PathBuf::from("/doc.md")),
            )
            .expect("the launch path resolves"),
        ),
        Arc::clone(&vfs),
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    let id = app.active;
    app.doc_mut(id).expect("doc exists").replica = Replica::Bound(DocDb::new(
        load.doc_id.0,
        PublishMode::OverwriteExisting,
        rune_db::Seq(0),
    ));
    app.install_or_join_file_binding(load.doc_id.0, load.saved_obs);

    // Simulates the state a lost materialize-record ack leaves behind
    // directly — reproducing the transient writer-queue failure for real
    // would make this test racy against the writer thread.
    app.file_binding_mut(load.doc_id.0)
        .expect("binding exists")
        .pending_rebaseline_hash = Some(rune_db::hash_bytes(b"hello"));

    // An external rewrite before the re-baseline load, so the fresh
    // observation asserted below is genuinely new, not incidentally
    // identical to the seed load's own.
    vfs.save_atomic(Path::new("/doc.md"), b"hello again")
        .expect("external rewrite");

    let doc_path = crate::resolved::ResolvedPath::resolve(app.vfs.as_ref(), Path::new("/doc.md"))
        .expect("Mem resolves any spelling");
    let enqueued = crate::db_enqueue::load_document_best_effort(&mut app, id, &doc_path);
    assert!(
        enqueued,
        "the re-baseline Load must enqueue against a live, non-degraded store"
    );

    let rebaseline_evt = bridge.wait_for_bootstrap_event(|evt| {
        matches!(
            evt,
            DbEvent::Ok {
                result: OpOutcome::Load(_),
                ..
            }
        )
    });
    let fresh_obs = match &rebaseline_evt {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => load.saved_obs.expect("a fresh load carries a baseline"),
        other => panic!("expected a Load ack, got {other:?}"),
    };

    let mut effects = crate::runtime::Effects::default();
    crate::app::update(
        &mut app,
        crate::runtime::Msg::Db(rebaseline_evt),
        &mut effects,
    );

    let binding = app
        .file_binding(load.doc_id.0)
        .expect("the shared binding must survive a binding_only re-baseline");
    assert_eq!(
        binding.expect_obs,
        Some(fresh_obs),
        "expect_obs must advance to the re-baseline Load's own fresh observation"
    );
    assert!(
        binding.pending_rebaseline_hash.is_none(),
        "a landed re-baseline must clear the stashed echo hash"
    );
}

fn app_bound_at_old_md(content: &str) -> (App, DocumentId, Arc<dyn Vfs + Send + Sync>) {
    let mem = Mem::new();
    mem.save_atomic(Path::new("/old.md"), content.as_bytes())
        .expect("seed old.md");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = DbBridge::bootstrap();
    let store =
        Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("open store");
    let old_path = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new("/old.md"))
        .expect("the launch path resolves");
    let mut app = App::new(
        Buffer::new(content),
        Some(old_path.clone()),
        Arc::clone(&vfs),
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    let id = app.active;
    assert!(
        crate::db_enqueue::load_document(
            &mut app,
            id,
            &old_path,
            crate::db_enqueue::LoadIntent::Recover
        ),
        "test setup: the open-time load enqueues"
    );
    pump_next_load_ack(&mut app);
    assert!(
        app.doc(id).expect("doc exists").is_store_bound(),
        "test setup: the tab is bound to its recovery row"
    );
    (app, id, vfs)
}

fn pump_next_load_ack(app: &mut App) {
    let evt = app
        .db
        .as_ref()
        .expect("store present")
        .bridge
        .wait_for_bootstrap_event(|evt| {
            matches!(
                evt,
                DbEvent::Ok {
                    result: OpOutcome::Load(_),
                    ..
                }
            )
        });
    let mut effects = crate::runtime::Effects::default();
    crate::app::update(app, crate::runtime::Msg::Db(evt), &mut effects);
}

#[test]
fn opening_the_new_name_of_an_externally_renamed_open_file_reunites_its_tab() {
    let (mut app, a, vfs) = app_bound_at_old_md("the user's words");
    vfs.rename_excl(Path::new("/old.md"), Path::new("/new.md"))
        .expect("external rename");

    let b = crate::workspace::open_path(&mut app, Path::new("/new.md")).expect("new.md opens");
    assert_ne!(a, b, "test setup: the open mints a fresh duplicate tab");
    pump_next_load_ack(&mut app);

    assert!(app.doc(b).is_none(), "the duplicate tab retires");
    assert_eq!(app.documents.len(), 1, "one tab remains for the one file");
    assert_eq!(app.active, a, "the surviving tab is focused");
    let new_path = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new("/new.md"))
        .expect("Mem resolves any spelling");
    assert_eq!(
        app.doc(a).expect("doc exists").resolved_path(),
        Some(&new_path),
        "the tab follows its file to the new name"
    );
    assert_eq!(
        crate::workspace::existing_document_for(&app, &new_path),
        Some(a)
    );
    assert_eq!(
        app.doc(a).expect("doc exists").buffer.content(),
        "the user's words"
    );
    assert_eq!(
        messages::newest_text(&app),
        Some("old.md was renamed to new.md on disk \u{2014} its open tab followed")
    );

    pump_next_load_ack(&mut app);
    assert_eq!(
        app.doc(a).expect("doc exists").buffer.content(),
        "the user's words",
        "the re-baseline load must never touch the buffer"
    );
    assert!(app.doc(a).expect("doc exists").is_store_bound());
}

#[test]
fn a_dirty_tab_at_the_old_name_keeps_both_tabs_and_warns() {
    let (mut app, a, vfs) = app_bound_at_old_md("the user's words");
    {
        let doc = app.doc_mut(a).expect("doc exists");
        doc.buffer = doc.buffer.insert(0, "!! ").expect("edit applies");
    }
    assert!(
        app.doc(a).expect("doc exists").is_dirty(),
        "test setup: the tab holds unsaved words"
    );
    vfs.rename_excl(Path::new("/old.md"), Path::new("/new.md"))
        .expect("external rename");

    let b = crate::workspace::open_path(&mut app, Path::new("/new.md")).expect("new.md opens");
    pump_next_load_ack(&mut app);

    assert!(app.doc(a).is_some(), "the dirty tab stays open");
    assert!(
        app.doc(b).is_some(),
        "nothing holding a fresh view is closed"
    );
    assert_eq!(
        app.doc(a).expect("doc exists").buffer.content(),
        "!! the user's words",
        "the dirty buffer is untouched"
    );
    assert_eq!(
        app.doc(a).expect("doc exists").path(),
        Some(Path::new("/old.md")),
        "nothing is rebound when unsaved words are involved"
    );
    let new_path = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new("/new.md"))
        .expect("Mem resolves any spelling");
    assert_eq!(
        crate::workspace::existing_document_for(&app, &new_path),
        Some(b)
    );
    assert_eq!(
        messages::newest_text(&app),
        Some(
            "old.md was renamed to new.md on disk \u{2014} it is open in another tab with unsettled changes, so both tabs were kept"
        )
    );
}
