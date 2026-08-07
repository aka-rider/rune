//! Work package A: a document that already has a `file_path` AND a
//! `DocDb { bind_new: true }` — a named file that does not exist on disk
//! yet, the shape a launch onto a not-yet-existing positional leaves
//! behind. `rename_common::unsaved_named_app_with_store` is the shared
//! fixture; the three end-to-end tests here drive it through the same
//! public entry points a user reaches (⌘S, `^R`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;
use std::sync::Arc;

use rune_tui::runtime::{CmdKind, Msg};

use rune_vfs::{Mem, Vfs};

use rename_common::{
    UNPUBLISHED_BODY, active_path, rename_to, send, sup, wait_for_load, wait_for_materialize_prep,
    wait_for_materialize_record,
};

/// ⌘S on a named-but-unpublished document creates the file with the
/// buffer's exact bytes, clears `bind_new`, and leaves the document clean.
#[test]
fn cmd_s_creates_the_file_and_clears_bind_new() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/seed.md"), b"seed")
        .expect("seed");
    let (mut app, bridge) = rename_common::unsaved_named_app_with_store(&mem);
    let id = app.active;

    send(&mut app, sup('s'));

    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    assert_eq!(
        mem.read(Path::new("/root/nope.md")).expect("file created"),
        UNPUBLISHED_BODY.as_bytes()
    );
    let doc_db = app.doc(id).unwrap().db.as_ref().expect("still bound");
    assert!(
        !doc_db.bind_new,
        "the create just committed — the next save is an overwrite"
    );
    assert!(
        !app.doc(id).unwrap().is_dirty(),
        "a just-published buffer must not read as dirty"
    );
}

/// ⌘S when the path is already occupied by other bytes (a create that
/// lost the race, A2): the other bytes survive untouched, one error is
/// posted, no `DiskConflict` guard is left standing, and the document
/// ends up bound to the file's own row with `bind_new == false`.
#[test]
fn cmd_s_on_a_lost_create_race_leaves_the_racers_bytes_and_rebinds() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/seed.md"), b"seed")
        .expect("seed");
    mem.save_atomic(Path::new("/root/nope.md"), b"racer bytes")
        .expect("a concurrent creator wins first");
    let (mut app, bridge) = rename_common::unsaved_named_app_with_store(&mem);
    let id = app.active;

    send(&mut app, sup('s'));

    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    // The A2 route hands the document off to an ordinary Load to install a
    // real CAS baseline instead of raising an unanswerable Guard.
    let load_evt = wait_for_load(&bridge);
    send(&mut app, Msg::Db(load_evt));

    assert_eq!(
        mem.read(Path::new("/root/nope.md")).expect("still there"),
        b"racer bytes",
        "the racer's bytes must survive untouched"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|m| m.contains("created by something")),
        "got {:?}",
        rune_tui::messages::newest_text(&app)
    );
    assert!(
        app.guard.is_none(),
        "a lost create race must never raise an unanswerable DiskConflict guard"
    );
    let doc_db = app.doc(id).unwrap().db.as_ref().expect("still bound");
    assert!(
        !doc_db.bind_new,
        "the document must come out of bind_new bound to the file's own row"
    );
}

/// `^R` + a new name on a never-published document creates the new name
/// in the document's OWN directory (A3), and the old name is never
/// created.
#[test]
fn rename_on_a_never_published_document_creates_at_the_new_name() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/seed.md"), b"seed")
        .expect("seed");
    mem.save_atomic(Path::new("/other.md"), b"unrelated")
        .expect("seed an unrelated root file so a root-joined target would be a real collision");
    let (mut app, bridge) = rename_common::unsaved_named_app_with_store(&mem);

    rename_to(&mut app, "fresh");

    // `rename::begin`'s create branch routes through `save::bind_new_now`,
    // the same store-backed materialize dance ⌘S uses: a `MaterializePrepare`
    // ack spawns the caller-side `vfs` `Cmd`, which itself replies with a
    // `Msg` that enqueues `MaterializeRecord`.
    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    let path = active_path(&app).expect("the document must now be bound");
    assert_eq!(
        path,
        Path::new("/root/fresh.md"),
        "must create next to the document's own directory, not the workspace root"
    );
    assert_eq!(mem.read(&path).unwrap(), UNPUBLISHED_BODY.as_bytes());
    assert!(
        mem.read(Path::new("/root/nope.md")).is_err(),
        "the old, never-published name must never be created"
    );
}
