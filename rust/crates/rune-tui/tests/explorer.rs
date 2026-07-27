//! WP4 "Done when" tests: Explorer navigation, dir loading, and
//! `workspace::open_path`, driven against a `Mem` vfs seeded with files and
//! nested directories.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, KeyOutcome, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::{explorer, workspace};
use rune_vfs::{Mem, Vfs};

/// Seeds a `Mem` vfs with `/root/a.md`, `/root/b.md`, and `/root/sub/c.md`
/// — two files plus a nested directory, so `Vfs::read_dir("/root")` lists
/// one dir ("sub") and two files ("a.md", "b.md").
fn seeded_vfs() -> Arc<Mem> {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/a.md"), b"a content")
        .expect("seed a.md");
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    mem.save_atomic(Path::new("/root/sub/c.md"), b"c content")
        .expect("seed sub/c.md");
    mem
}

/// An `App` whose active document is `/root/a.md` — so the Explorer's
/// `initial_root` (the active document's own directory) resolves to
/// `/root`.
fn app_with(mem: &Arc<Mem>) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new("a content"),
        Some(PathBuf::from("/root/a.md")),
        vfs,
        None,
    );
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();
    app
}

fn ctrl_x() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('x'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    }
}

fn key(code: KeyCode) -> KeyInput {
    KeyInput {
        code,
        mods: Mods::NONE,
    }
}

/// `^x` through the real `update`, then runs the one `ReadDir` `Cmd` it
/// enqueues and delivers its `Msg::DirLoaded` reply — the same two-step
/// production actually performs across the `Cmd` thread boundary, just
/// synchronously here.
fn load_explorer(app: &mut App) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(ctrl_x()), &mut effects);
    assert_eq!(effects.cmds.len(), 1, "^x must enqueue exactly one Cmd");
    assert_eq!(effects.cmds[0].kind(), CmdKind::ReadDir);
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects2 = Effects::default();
    app::update(app, msg, &mut effects2);
}

#[test]
fn ctrl_x_populates_the_explorer_via_dir_loaded() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    load_explorer(&mut app);

    assert!(app.left_visible);
    assert_eq!(app.focus, Pane::Explorer);
    assert!(!app.explorer.loading);
    assert_eq!(app.explorer.root, PathBuf::from("/root"));

    let names: Vec<&str> = app
        .explorer
        .entries
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, vec!["sub", "a.md", "b.md"], "dirs first, then names");
}

#[test]
fn up_and_down_clamp_at_the_list_bounds() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    let mut effects = Effects::default();

    assert_eq!(
        explorer::handle_key(&mut app, key(KeyCode::Up), &mut effects),
        KeyOutcome::Consumed
    );
    assert_eq!(app.explorer.nav.cursor, 0, "clamped at the top");

    for _ in 0..10 {
        let outcome = explorer::handle_key(&mut app, key(KeyCode::Down), &mut effects);
        assert_eq!(outcome, KeyOutcome::Consumed);
    }
    assert_eq!(
        app.explorer.nav.cursor,
        app.explorer.entries.len() - 1,
        "clamped at the bottom"
    );
}

#[test]
fn enter_on_a_file_opens_a_second_document_and_focuses_editor() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    let idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "b.md")
        .expect("b.md listed");
    app.explorer.nav.cursor = idx;
    let before_docs = app.documents.len();

    let mut effects = Effects::default();
    let outcome = explorer::handle_key(&mut app, key(KeyCode::Enter), &mut effects);

    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(app.documents.len(), before_docs + 1);
    assert_eq!(app.focus, Pane::Editor);
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/b.md"))
    );
    assert_eq!(app.active_doc().buffer.content(), "b content");
}

#[test]
fn enter_on_a_directory_issues_a_read_dir_cmd() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    let idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "sub")
        .expect("sub listed");
    app.explorer.nav.cursor = idx;
    let before_docs = app.documents.len();

    let mut effects = Effects::default();
    let outcome = explorer::handle_key(&mut app, key(KeyCode::Enter), &mut effects);

    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::ReadDir);
    assert!(app.explorer.loading);
    assert_eq!(
        app.documents.len(),
        before_docs,
        "opening a dir must not create a Document"
    );
}

#[test]
fn refresh_cause_preserves_the_selected_entry_by_name() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    let idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "b.md")
        .expect("b.md listed");
    app.explorer.nav.cursor = idx;

    let root = app.explorer.root.clone();
    let generation = app.explorer.request_generation;
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::DirLoaded {
            root,
            generation,
            entries: vec![
                rune_vfs::DirEntry {
                    name: "new.md".to_string(),
                    is_dir: false,
                },
                rune_vfs::DirEntry {
                    name: "a.md".to_string(),
                    is_dir: false,
                },
                rune_vfs::DirEntry {
                    name: "b.md".to_string(),
                    is_dir: false,
                },
                rune_vfs::DirEntry {
                    name: "sub".to_string(),
                    is_dir: true,
                },
            ],
            cause: rune_tui::runtime::DirCause::Refresh,
        },
        &mut effects,
    );

    assert_eq!(app.explorer.entries[app.explorer.nav.cursor].name, "b.md");
}

/// Two in-flight `ReadDir` Cmds landing out of order (Backspace to the
/// parent, then immediately Enter into a different child, with the OLDER
/// reply arriving second) must not let the stale reply win — review fix for
/// `explorer::handle_dir_loaded`'s missing generation guard.
#[test]
fn an_out_of_order_stale_dir_loaded_reply_is_ignored() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    let stale_generation = app.explorer.request_generation;

    // Issue a second `ReadDir` (Backspace to the parent) — bumps the
    // generation without yet delivering a reply.
    let mut effects = Effects::default();
    let outcome = explorer::handle_key(&mut app, key(KeyCode::Backspace), &mut effects);
    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(effects.cmds.len(), 1);
    let fresh_root = app.explorer.root.clone();
    assert_ne!(
        app.explorer.request_generation, stale_generation,
        "test setup: the second request must bump the generation"
    );

    // The FIRST (now-stale) request's reply arrives late.
    let mut effects2 = Effects::default();
    app::update(
        &mut app,
        Msg::DirLoaded {
            root: PathBuf::from("/stale/should-not-apply"),
            entries: vec![rune_vfs::DirEntry {
                name: "should-not-appear".to_string(),
                is_dir: false,
            }],
            cause: rune_tui::runtime::DirCause::Nav,
            generation: stale_generation,
        },
        &mut effects2,
    );

    assert_eq!(
        app.explorer.root, fresh_root,
        "a stale-generation reply must not overwrite the newer in-flight request's root"
    );
    assert_ne!(app.explorer.root, PathBuf::from("/stale/should-not-apply"));
}

#[test]
fn open_path_on_an_already_open_document_reactivates_instead_of_duplicating() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let before = app.documents.len();
    let first_active = app.active;
    app.focus = Pane::Explorer;

    workspace::open_path(&mut app, Path::new("/root/a.md"));

    assert_eq!(app.documents.len(), before, "must not duplicate");
    assert_eq!(app.active, first_active);
    assert_eq!(app.focus, Pane::Editor);
}
