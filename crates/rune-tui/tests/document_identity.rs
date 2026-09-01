#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;
mod rename_common;

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{DbEvent, OpOutcome};
use rune_fuzz::Session;
use rune_tui::app::{self, App};
use rune_tui::db::{LoadPurpose, PendingOp};
use rune_tui::resolved::ResolvedPath;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs, VfsTestExt};

use merge_common::{ch, drain_materialize_round_trip, sup};
use rename_common::{app_with, rename_to, seeded_vfs, send};

fn app_on(mem: &Arc<Mem>, path: &str) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let launch = ResolvedPath::resolve(vfs.as_ref(), Path::new(path)).expect("the path resolves");
    App::new(Buffer::new("seed"), Some(launch), vfs, None)
}

#[test]
fn reopening_after_save_reactivates_the_same_tab_because_identity_is_the_path_not_the_inode() {
    let mut session = Session::open("/doc.md", "hello");
    let id = session.app().active;

    assert!(session.key(ch('!')).is_none());
    assert!(session.key(sup('s')).is_none());
    drain_materialize_round_trip(&mut session);

    let tabs_before = session.app().documents.len();
    let reopened = workspace::open_path(session.app_mut(), Path::new("/doc.md"));

    assert_eq!(reopened, Some(id), "the saved file is still this tab");
    assert_eq!(
        session.app().documents.len(),
        tabs_before,
        "a save must never make the same file openable twice"
    );
}

#[test]
fn an_external_rename_reported_via_renamed_from_reunites_the_tab_under_the_new_name() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/old.md"), b"seed")
        .expect("seed old.md");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let bridge = rune_tui::db::DbBridge::bootstrap();
    let store = rune_db::Store::open_in_memory(
        Arc::new(std::time::SystemTime::now),
        Arc::clone(&vfs),
        bridge.on_event(),
    )
    .expect("open store");
    let launch = ResolvedPath::resolve(vfs.as_ref(), Path::new("/old.md"))
        .expect("the launch path resolves");
    let mut app = App::new(
        Buffer::new("seed"),
        Some(launch),
        vfs,
        Some(rune_tui::db::Db::new(store, Arc::clone(&bridge), false)),
    );
    let id = app.active;
    let issued_version = app.doc(id).unwrap().buffer.version();
    let op_id = app
        .db
        .as_ref()
        .unwrap()
        .store
        .load(Path::new("/old.md"))
        .expect("enqueue the open-time load");
    app.db_ops.insert(
        op_id,
        PendingOp::load(id, issued_version, LoadPurpose::Recover),
    );
    pump_load_ack(&mut app, &bridge);
    assert!(app.doc(id).unwrap().is_store_bound(), "the tab is bound");

    mem.rename_excl(Path::new("/old.md"), Path::new("/new.md"))
        .expect("external rename");
    let duplicate = workspace::open_path(&mut app, Path::new("/new.md")).expect("new.md opens");
    assert_ne!(duplicate, id, "the open first mints a fresh duplicate tab");
    pump_load_ack(&mut app, &bridge);

    assert_eq!(
        app.doc(id).unwrap().path(),
        Some(Path::new("/new.md")),
        "the tab must follow the file to its new name"
    );
    assert!(app.doc(duplicate).is_none(), "the duplicate retires");
    let tabs_before = app.documents.len();
    assert_eq!(
        workspace::open_path(&mut app, Path::new("/new.md")),
        Some(id),
        "the new name is this tab"
    );
    assert_eq!(app.documents.len(), tabs_before, "and mints no second tab");
}

fn pump_load_ack(app: &mut App, bridge: &rune_tui::db::DbBridge) {
    let evt = bridge.wait_for_bootstrap_event(|evt| {
        matches!(
            evt,
            DbEvent::Ok {
                result: OpOutcome::Load(_),
                ..
            }
        )
    });
    let mut effects = Effects::default();
    app::update(app, Msg::Db(evt), &mut effects);
}

#[test]
fn two_hardlinked_paths_stay_two_documents_on_purpose() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/one.md"), b"shared bytes")
        .expect("seed one.md");
    mem.save_atomic(Path::new("/two.md"), b"shared bytes")
        .expect("seed two.md");
    mem.set_nlink(Path::new("/one.md"), 2)
        .expect("one.md is hard-linked");
    mem.set_nlink(Path::new("/two.md"), 2)
        .expect("two.md is hard-linked");
    let mut app = app_on(&mem, "/one.md");

    let first = workspace::open_path(&mut app, Path::new("/one.md")).expect("one.md is open");
    let second = workspace::open_path(&mut app, Path::new("/two.md")).expect("two.md opens");

    assert_ne!(
        first, second,
        "each name of a hard-linked file is its own document"
    );
    assert_eq!(app.doc(first).unwrap().path(), Some(Path::new("/one.md")));
    assert_eq!(app.doc(second).unwrap().path(), Some(Path::new("/two.md")));
}

#[test]
fn a_rename_moves_the_path_so_a_later_file_under_the_old_name_is_a_different_document() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let id = app.active;

    let mut effects = rename_to(&mut app, "b");
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");
    send(&mut app, cmd.run().expect("a reply"));

    assert_eq!(
        app.doc(id).unwrap().path(),
        Some(Path::new("/root/b.md")),
        "the document answers to its new name"
    );
    assert_eq!(
        workspace::open_path(&mut app, Path::new("/root/b.md")),
        Some(id),
        "opening the new name reactivates it"
    );

    mem.save_atomic(Path::new("/root/a.md"), b"a fresh file")
        .expect("something else takes the freed name");
    let reused = workspace::open_path(&mut app, Path::new("/root/a.md")).expect("the new a.md");

    assert_ne!(
        reused, id,
        "the old name must no longer name the renamed document"
    );
}

#[test]
fn closing_a_document_frees_its_path_so_reopening_it_mints_a_fresh_document() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/one.md"), b"one")
        .expect("seed one.md");
    let mut app = app_on(&mem, "/anchor.md");
    let opened = workspace::open_path(&mut app, Path::new("/one.md")).expect("one.md opens");
    let tabs_with_it_open = app.documents.len();

    let mut effects = Effects::default();
    assert!(matches!(
        workspace::close_now(&mut app, opened, &mut effects),
        workspace::CloseOutcome::Closed
    ));

    assert!(app.doc(opened).is_none(), "the tab is gone");

    let reopened = workspace::open_path(&mut app, Path::new("/one.md")).expect("one.md reopens");

    assert_ne!(reopened, opened, "reopening mints a fresh document");
    assert_eq!(
        app.documents.len(),
        tabs_with_it_open,
        "and exactly one tab holds the file again"
    );
    assert_eq!(
        app.doc(reopened).unwrap().path(),
        Some(Path::new("/one.md"))
    );
}
