//! Reproductions for #119 (two tabs sharing one recovery row silently
//! mis-resolve each other's undo positions) and #120 (an external disk
//! change reanchored under a clean buffer leaves a live tab's undo mapping
//! off by the bridge entry it never learns about). Both drive a bare `App`
//! over a real file-backed `Store` so a restart proves what crash recovery
//! actually reconstructs — the same idiom `db_wiring_undo_rebase.rs` uses.
//! Neither test asserts on `DocDb`'s private counters (`undo_offset`/
//! `undo_floor`/`appends_sent` are `pub(crate)`, unreachable from here
//! anyway): both assert on what a restarted session recovers, which is the
//! only way a wrong undo resolution is ever user-visible.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

#[path = "db_wiring_common/mod.rs"]
mod db_wiring_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{DbEvent, OpOutcome, Seq, Store};
use rune_tui::app::App;
use rune_tui::db::{Db, DbBridge, DocDb, PublishMode};
use rune_tui::db_enqueue::{self, LoadIntent};
use rune_tui::document::DocumentId;
use rune_tui::keymap::KeyCode;
use rune_tui::runtime::Msg;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{publish, restarted_store_at, temp_db_dir};
use rename_common::{plain, send, type_text, wait_for_load};

const DOC: &str = "/root/a.md";

/// `merge_common::external_write`'s shape, generalized off the hardcoded
/// `/doc.md` there: a real external rewrite of a file that already exists,
/// which `db_wiring_common::publish`'s `rename_excl` alone refuses (it is
/// create-only).
fn external_write(vfs: &dyn Vfs, path: &Path, bytes: &[u8]) {
    vfs.remove(path).expect("remove the stale file");
    publish(vfs, path, bytes);
}

fn file_store_app(mem: &Arc<Mem>, db_path: &Path, content: &str) -> (App, Arc<DbBridge>) {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let bridge = DbBridge::bootstrap();
    let (store, warning) = Store::open(db_path, Arc::clone(&vfs), bridge.on_event()).expect("open");
    assert!(warning.is_none(), "the open ladder must not degrade");

    store.load(Path::new(DOC)).expect("enqueue load");
    let load = match bridge.wait_for_bootstrap_event(|_| true) {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => *load,
        other => panic!("expected a Load ack, got {other:?}"),
    };

    let mut app = App::new(
        Buffer::new(content),
        Some(PathBuf::from(DOC)),
        vfs,
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    app.active_doc_mut().set_doc_db_for_test(DocDb::new(
        load.doc_id.0,
        PublishMode::OverwriteExisting,
        Seq(0),
    ));
    app.install_or_join_file_binding(load.doc_id.0, load.saved_obs);
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();
    (app, bridge)
}

/// The two-tabs-one-row shape `g7_shared_file_baseline.rs`'s own
/// `bind_second_tab` establishes — copied here (bare `App`, not
/// `rune_fuzz::Session`) since this repro needs a real file-backed store
/// a restart can prove what it actually reconstructs. No user-reachable
/// open path reaches this precondition (`bind_second_tab`'s own doc
/// comment): every real second open of an already-open path deduplicates
/// to a reactivation of the existing tab.
fn bind_second_tab(app: &mut App, db_id: i64, path: &Path, content: &str) -> DocumentId {
    let id = app.open_document(Buffer::new(content));
    {
        let doc = app.doc_mut(id).unwrap();
        doc.file_path = Some(path.to_path_buf());
        doc.set_doc_db_for_test(DocDb::new(db_id, PublishMode::OverwriteExisting, Seq(0)));
    }
    app.install_or_join_file_binding(db_id, None);
    id
}

fn drain_db_ops(app: &mut App, bridge: &Arc<DbBridge>) {
    while let Some(&op_id) = app.db_ops.keys().min() {
        let evt = bridge.wait_for_bootstrap_event(|evt| match evt {
            DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
            DbEvent::Fatal { .. } => true,
        });
        send(app, Msg::Db(evt));
    }
}

fn restart_recovered(mem: &Arc<Mem>, db_path: &Path) -> String {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let (store_b, bridge_b) = restarted_store_at(db_path, vfs);
    let op_id = store_b.load(Path::new(DOC)).expect("enqueue load");
    let load_evt = bridge_b.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    });
    store_b.shutdown();
    match load_evt {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => load.recovered,
        other => panic!("a fresh session's load must not fail, got {other:?}"),
    }
}

/// #119: tab A and tab B are two `Document`s bound to the SAME recovery
/// row. B writes its own edit first, then A writes two edits of its own.
/// A's own local undo journal counts only A's own edits — position 1 is
/// "right after my first edit, before my second" — but the writer thread's
/// `DocUndoState` numbers `AppendEdit`s across BOTH tabs in landing order,
/// so A's position-1 resolves to the writer's index 0, which is B's edit,
/// not A's own first edit. A one-step undo in A must only drop A's own
/// second edit; it must never resolve to a state that has dropped A's own
/// first edit too, durable history from before it included or not.
#[test]
#[ignore = "reproduces #119"]
fn undo_in_one_tab_never_resolves_past_its_own_first_edit_when_a_sibling_tab_shares_the_row() {
    let dir = temp_db_dir("shared-row-undo-drift");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new(DOC), b"");
    let (mut app, bridge) = file_store_app(&mem, &db_path, "");
    let id_a = app.active;

    let db_id = app.doc(id_a).unwrap().doc_db().unwrap().db_id;
    let id_b = bind_second_tab(&mut app, db_id, Path::new(DOC), "");

    workspace::switch_to(&mut app, id_b);
    type_text(&mut app, "\u{3b2}");
    drain_db_ops(&mut app, &bridge);

    workspace::switch_to(&mut app, id_a);
    type_text(&mut app, "\u{3b1}");
    drain_db_ops(&mut app, &bridge);
    type_text(&mut app, "\u{3c9}");
    drain_db_ops(&mut app, &bridge);

    assert_eq!(app.doc(id_a).unwrap().buffer.content(), "\u{3b1}\u{3c9}");
    assert_eq!(app.doc(id_a).unwrap().journal.pos(), 2);

    rune_tui::commands::edit::undo(&mut app, id_a);
    assert_eq!(
        app.doc(id_a).unwrap().buffer.content(),
        "\u{3b1}",
        "tab A's own local buffer must show only its first edit after one undo"
    );
    drain_db_ops(&mut app, &bridge);
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "the undo must not degrade the store"
    );

    app.db.take().expect("store wired").shutdown();
    let recovered = restart_recovered(&mem, &db_path);
    assert_eq!(
        recovered, "\u{3b2}\u{3b1}",
        "crash recovery after A's one-step undo must reconstruct B's edit \
         followed by A's own surviving first edit — got {recovered:?}, which \
         proves the resolved durable position lost A's own first edit even \
         though the undo was only meant to drop A's second"
    );
}

/// #120: a clean tab (no local edits, buffer matches its last-saved
/// observation) sits on a row whose file changes externally. The reload
/// that follows adopts the new disk content into the row's durable
/// reconstruction via a bridge `reanchor_clean_reload_tx` journals, but
/// `disk_content == recovered` makes `Document::hydrate` return `NoChange`
/// — the buffer is never refreshed, and `bind_document_row`'s same-row arm
/// never computes a coordinate re-base for the bridge the way its
/// different-db_id arm does. The tab's very next edit is sent with
/// coordinates that assume its OWN (stale) buffer content, not the row's
/// actual bridged reconstruction, so it lands at the wrong offset in the
/// durable history — a silent splice, not the edit the user made.
#[test]
#[ignore = "reproduces #120"]
fn an_edit_right_after_a_clean_external_reload_lands_at_the_wrong_offset_in_recovery() {
    let dir = temp_db_dir("clean-reload-bridge-drift");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new(DOC), b"hello");
    let (mut app, bridge) = file_store_app(&mem, &db_path, "hello");
    let id = app.active;

    assert_eq!(
        app.doc(id).unwrap().journal.pos(),
        0,
        "test setup: a clean tab"
    );

    external_write(mem.as_ref(), Path::new(DOC), b"1234567890");

    assert!(
        db_enqueue::load_document(&mut app, id, Path::new(DOC), LoadIntent::Recover),
        "the reload must enqueue"
    );
    let load_evt = wait_for_load(&bridge);
    send(&mut app, Msg::Db(load_evt));

    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "hello",
        "test setup: a clean-hydration NoChange leaves the buffer exactly as it was"
    );

    send(&mut app, plain(KeyCode::End));
    type_text(&mut app, "!");
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello!");
    drain_db_ops(&mut app, &bridge);
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "the post-reload edit must not degrade the store"
    );

    app.db.take().expect("store wired").shutdown();
    let recovered = restart_recovered(&mem, &db_path);
    assert_eq!(
        recovered, "1234567890!",
        "the tab's post-reload edit must extend the row's actual bridged \
         reconstruction (the externally written disk content), not splice \
         into it at an offset borrowed from the tab's own stale buffer \
         coordinates — got {recovered:?}"
    );
}
