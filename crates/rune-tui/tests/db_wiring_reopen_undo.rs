//! Regression: reopening a tab in the same session writes no bridge, so the
//! fresh binding's local undo position 0 must resolve to the durable position
//! the reopened content actually sits at — never to journal seq 0. Driven
//! through `rune_fuzz::Session`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;
use std::sync::Arc;

use rune_db::{DbEvent, OpOutcome};
use rune_fuzz::Session;
use rune_fuzz::driver::wait_for_db_op;
use rune_tui::db::Db;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::Effects;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{publish, restarted_store_at, store_at, temp_db_dir};

const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};

const END: KeyInput = KeyInput {
    code: KeyCode::End,
    mods: Mods::NONE,
};

const SAVE: KeyInput = KeyInput {
    code: KeyCode::Char('s'),
    mods: SUP,
};

const UNDO: KeyInput = KeyInput {
    code: KeyCode::Char('z'),
    mods: SUP,
};

/// Edit, save, close, reopen, type, undo back to the reopened content, then
/// crash: recovery must rebuild the reopened content. Mapping the reopened
/// binding's local position 0 to durable seq 0 instead rebuilds the first
/// load's content and lets the next append truncate every later event.
#[test]
fn undo_to_the_start_of_a_reopened_tab_recovers_the_reopened_content() {
    let dir = temp_db_dir("reopen-undo");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), doc_path, b"hello");
    publish(mem.as_ref(), Path::new("/other.md"), b"other");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    let (store, bridge) = store_at(&db_path, Arc::clone(&vfs));
    let mut session =
        Session::open_with_db("/doc.md", Arc::clone(&mem), Db::new(store, bridge, false));
    let first_tab = session.app().active;

    assert!(session.key(END).is_none());
    assert!(session.type_(" world").is_none());
    assert!(session.deliver_db_all().is_none());
    assert!(session.key(SAVE).is_none());
    assert!(session.deliver_db_all().is_none());
    assert!(session.deliver().is_none());
    assert!(session.deliver_db_all().is_none());
    assert_eq!(mem.read(doc_path).expect("saved file"), b"hello world");

    workspace::open_path(session.app_mut(), Path::new("/other.md")).expect("open other");
    assert!(session.deliver_db_all().is_none());
    let _ = workspace::close_now(session.app_mut(), first_tab, &mut Effects::default());
    workspace::open_path(session.app_mut(), doc_path).expect("reopen doc");
    assert!(session.deliver_db_all().is_none());
    assert_eq!(session.snapshot().content, "hello world");

    assert!(session.type_("!").is_none());
    assert!(session.deliver_db_all().is_none());
    assert_eq!(session.snapshot().content, "!hello world");
    assert!(session.key(UNDO).is_none());
    assert!(session.deliver_db_all().is_none());
    assert_eq!(session.snapshot().content, "hello world");

    session
        .app_mut()
        .db
        .take()
        .expect("session has a store")
        .shutdown();
    drop(session);

    let (store_b, bridge_b) = restarted_store_at(&db_path, Arc::clone(&vfs));
    let op_id = store_b.load(doc_path).expect("enqueue load");
    let load_b = match wait_for_db_op(&bridge_b, op_id) {
        DbEvent::Ok {
            result: OpOutcome::Load(r),
            ..
        } => *r,
        other => panic!("expected a Load ack, got {other:?}"),
    };
    store_b.shutdown();

    assert_eq!(
        load_b.recovered.content, "hello world",
        "recovery must rebuild the content the reopened tab undid back to"
    );
}
