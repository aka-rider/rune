#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, Vfs, VfsTestExt};

use crate::app::App;
use crate::runtime::{CmdKind, Effects, Msg};

use super::{Commit, RenameState};

#[test]
fn closing_a_document_mid_rename_still_reports_the_acks_outcome() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/old.md"), b"hello")
        .expect("seed old.md");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new("hello"),
        Some(
            crate::resolved::ResolvedPath::resolve(
                vfs.as_ref(),
                std::path::Path::new(&PathBuf::from("/old.md")),
            )
            .expect("the launch path resolves"),
        ),
        vfs,
        None,
    );
    let id = app.active;
    app.title.set_text("new.md");

    let mut effects = Effects::default();
    assert_eq!(super::begin(&mut app, &mut effects), Commit::Accepted);
    assert!(app.rename.in_flight(), "test setup: a rename is in flight");

    let outcome = crate::workspace::close_now(&mut app, id, &mut effects);
    assert!(matches!(outcome, crate::workspace::CloseOutcome::Closed));
    assert!(app.doc(id).is_none(), "the doc really closed");
    assert!(
        app.rename.in_flight(),
        "closing the doc must not cancel the rename already in flight"
    );

    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("begin spawns the no-store rename Cmd");
    let Msg::RenameDone { generation, result } = cmd.run().expect("the rename Cmd replies") else {
        panic!("expected Msg::RenameDone");
    };
    super::handle_rename_done(&mut app, generation, result, &mut effects);

    assert!(
        matches!(app.rename, RenameState::Idle),
        "the ack must resolve the machine even though the doc is gone"
    );
    assert_eq!(
        mem.read(Path::new("/new.md")).expect("read new.md"),
        b"hello",
        "the rename itself must still land on disk"
    );
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("renamed to new.md"),
        "the outcome must still be reported even though the tab already closed"
    );
}

#[test]
fn a_no_store_draft_create_advances_the_dirty_baseline_to_what_was_written() {
    let mem = Arc::new(Mem::new());
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new(""), None, vfs, None);
    app.set_root(PathBuf::from("/"));
    let id = app.active;
    crate::commands::edit::insert_char(&mut app, id, 'X');
    assert!(
        app.doc(id).expect("doc open").is_dirty(),
        "test setup: typing must dirty the draft"
    );
    app.title.set_text("new.md");

    let mut effects = Effects::default();
    assert_eq!(super::begin(&mut app, &mut effects), Commit::Accepted);
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("bind_new spawns the no-store create Cmd");
    let Msg::RenameDone { generation, result } = cmd.run().expect("the create Cmd replies") else {
        panic!("expected Msg::RenameDone");
    };
    super::handle_rename_done(&mut app, generation, result, &mut effects);

    assert_eq!(
        mem.read(Path::new("/new.md")).expect("read new.md"),
        b"X",
        "test setup: the create must have actually published the typed byte"
    );
    assert!(
        !app.doc(id).expect("doc open").is_dirty(),
        "a file that byte-matches what was just written must not read as unsaved"
    );
}

#[test]
fn a_tab_whose_file_another_tab_is_renamed_onto_keeps_its_unsaved_words() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/x.md"), b"x body")
        .expect("seed x.md");
    mem.save_atomic(Path::new("/y.md"), b"y body")
        .expect("seed y.md");
    let vfs: Arc<dyn Vfs + Send + Sync> = mem;
    let x = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new("/x.md"))
        .expect("/x.md resolves");
    let y = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new("/y.md"))
        .expect("/y.md resolves");
    let mut app = App::new(
        Buffer::new("x body"),
        Some(x.clone()),
        Arc::clone(&vfs),
        None,
    );
    let a = app.active;
    let b = app.open_document_bound(Buffer::new("y body"), y.clone());
    crate::commands::edit::insert_char(&mut app, a, 'Z');
    let typed = app.doc(a).expect("a is open").buffer.content().to_string();

    app.rebind_document_path(b, x.clone());

    let loser = app.doc(a).expect("the tab that lost its file stays open");
    assert_eq!(loser.buffer.content(), typed, "the unsaved words survive");
    assert!(loser.is_dirty(), "the unsaved edit still reads as unsaved");
    assert_eq!(loser.file_name(), "x.md", "the tab keeps the name it had");
    assert_eq!(
        loser.path(),
        None,
        "the tab no longer answers for the file it lost"
    );
    assert_eq!(
        crate::workspace::existing_document_for(&app, &x),
        Some(b),
        "the tab that took the file over is the one the path resolves to"
    );
}

#[test]
fn begin_refuses_a_rename_while_a_trash_is_in_flight() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/old.md"), b"hello")
        .expect("seed old.md");
    let vfs: Arc<dyn Vfs + Send + Sync> = mem;
    let mut app = App::new(
        Buffer::new("hello"),
        Some(
            crate::resolved::ResolvedPath::resolve(
                vfs.as_ref(),
                std::path::Path::new(&PathBuf::from("/old.md")),
            )
            .expect("the launch path resolves"),
        ),
        vfs,
        None,
    );
    app.trash = crate::trash::TrashState::Pending {
        generation: app.next_trash_gen.mint(),
    };
    app.title.set_text("new.md");

    let mut effects = Effects::default();
    assert_eq!(super::begin(&mut app, &mut effects), Commit::Refused);
    assert!(matches!(app.rename, RenameState::Idle));
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("can't rename while a trash is in progress")
    );
}
