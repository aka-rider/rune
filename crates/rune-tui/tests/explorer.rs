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
use rune_tui::{explorer, explorer_keys, workspace};
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

fn ctrl_b() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('b'),
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

/// `^b` through the real `update`, then runs the one `ReadDir` `Cmd` it
/// enqueues and delivers its `Msg::DirLoaded` reply — the same two-step
/// production actually performs across the `Cmd` thread boundary, just
/// synchronously here.
fn load_explorer(app: &mut App) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(ctrl_b()), &mut effects);
    assert_eq!(effects.cmds.len(), 1, "^b must enqueue exactly one Cmd");
    assert_eq!(effects.cmds[0].kind(), CmdKind::ReadDir);
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects2 = Effects::default();
    app::update(app, msg, &mut effects2);
}

#[test]
fn ctrl_b_populates_the_explorer_via_dir_loaded() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    load_explorer(&mut app);

    assert!(app.splits.left.is_shown());
    assert_eq!(app.focus(), Pane::Explorer);
    assert!(!app.explorer.loading);
    assert_eq!(app.explorer.root, PathBuf::from("/root"));

    let names: Vec<&str> = app
        .explorer
        .entries
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["..", "sub", "a.md", "b.md"],
        "leading '..' row (root has a parent), then dirs, then names"
    );
}

#[test]
fn up_and_down_clamp_at_the_list_bounds() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    let mut effects = Effects::default();

    assert_eq!(
        explorer_keys::handle_key(&mut app, key(KeyCode::Up), &mut effects),
        KeyOutcome::Consumed
    );
    assert_eq!(app.explorer.nav.cursor, 0, "clamped at the top");

    for _ in 0..10 {
        let outcome = explorer_keys::handle_key(&mut app, key(KeyCode::Down), &mut effects);
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
    let outcome = explorer_keys::handle_key(&mut app, key(KeyCode::Enter), &mut effects);

    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(app.documents.len(), before_docs + 1);
    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/b.md"))
    );
    assert_eq!(app.active_doc().buffer.content(), "b content");
}

/// WP13.S1 (finding `rune-tui C 1`): a directory entry whose filename is
/// not valid UTF-8 must still open the RIGHT file. `entry.name` is
/// necessarily lossy (may collapse to U+FFFD), so if `open_selected`
/// rejoined `root.join(&entry.name)` — the pre-fix code — it would target
/// a path the user's file was never actually saved at. Opening instead
/// through `entry.path` (byte-exact, straight from `Mem`'s own key) must
/// reach the real file and its real bytes.
#[test]
fn enter_on_a_non_utf8_named_file_opens_the_byte_exact_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let mem = seeded_vfs();
    let raw_name = OsStr::from_bytes(b"caf\xE9.md"); // invalid UTF-8
    let raw_path = PathBuf::from("/root").join(raw_name);
    mem.save_atomic(&raw_path, b"non-utf8-named content")
        .expect("seed the non-UTF-8 named file");

    let mut app = app_with(&mem);
    load_explorer(&mut app);
    let idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name.contains('\u{FFFD}'))
        .expect("the non-UTF-8 name is listed, lossily, as containing U+FFFD");
    assert_eq!(
        app.explorer.entries[idx].path, raw_path,
        "the entry's `path` must be the byte-exact key, not the lossy name"
    );
    app.explorer.nav.cursor = idx;

    let mut effects = Effects::default();
    let outcome = explorer_keys::handle_key(&mut app, key(KeyCode::Enter), &mut effects);

    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(raw_path.as_path()),
        "the opened document's path must be the real, byte-exact path"
    );
    assert_eq!(app.active_doc().buffer.content(), "non-utf8-named content");
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
    let outcome = explorer_keys::handle_key(&mut app, key(KeyCode::Enter), &mut effects);

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

/// The leading ".." row is a REAL `DirEntry` carrying the real parent path
/// (`with_parent_entry`, `explorer.rs`) — so Enter on it needs no special
/// case: it flows through `open_selected`'s ordinary directory branch, which
/// issues a `ReadDir` `Cmd` for the parent, exactly like Backspace's
/// `go_to_parent` does.
#[test]
fn enter_on_the_parent_row_loads_the_parent_directory() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);

    assert_eq!(app.explorer.entries[0].name, "..");
    assert_eq!(app.explorer.nav.cursor, 0);

    let mut effects = Effects::default();
    let outcome = explorer_keys::handle_key(&mut app, key(KeyCode::Enter), &mut effects);

    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::ReadDir);
    assert!(app.explorer.loading);

    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);
    assert_eq!(app.explorer.root, PathBuf::from("/"));
}

/// A root with no parent (a filesystem root) gets no synthetic ".." row —
/// `with_parent_entry`'s `root.parent() == None` branch.
#[test]
fn a_root_without_a_parent_gets_no_dotdot_row() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);

    // Navigate up to the vfs's own synthetic root ("/"), which has no parent.
    let mut effects = Effects::default();
    let outcome = explorer_keys::handle_key(&mut app, key(KeyCode::Backspace), &mut effects);
    assert_eq!(outcome, KeyOutcome::Consumed);
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);
    assert_eq!(app.explorer.root, PathBuf::from("/"));

    assert_ne!(
        app.explorer.entries.first().map(|e| e.name.as_str()),
        Some(".."),
        "the filesystem root has no parent, so no '..' row is injected"
    );
}

/// Review fix: `open_selected`'s directory branch must resolve the new root
/// through `app.vfs.resolve` (§1.4.9), same as `initial_root` already does
/// — and, on a `resolve` error, fall back to the unresolved path (the same
/// pattern `workspace::open_path` uses) rather than losing the navigation.
/// `Mem::resolve` is an identity function, so this can't observe a resolved
/// path actually DIFFERING from the raw one — it proves the fallback is
/// exercised (no panic, the `ReadDir` Cmd still fires, the listing still
/// lands) when `resolve` itself fails.
#[test]
fn open_selected_on_a_directory_falls_back_when_resolve_fails() {
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

    // Armed AFTER the initial `^b` load (which itself resolves
    // `initial_root`) so it targets THIS Enter-on-a-directory's own
    // `resolve` call, not an earlier, unrelated one.
    mem.fail_next(rune_vfs::OpKind::Resolve, std::io::ErrorKind::Other);

    let mut effects = Effects::default();
    let outcome = explorer_keys::handle_key(&mut app, key(KeyCode::Enter), &mut effects);
    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(
        effects.cmds.len(),
        1,
        "a resolve failure must not drop the ReadDir Cmd"
    );

    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);
    assert_eq!(app.explorer.root, PathBuf::from("/root/sub"));
}

/// Same fallback guarantee for Backspace's `go_to_parent`.
#[test]
fn go_to_parent_falls_back_when_resolve_fails() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    assert_eq!(app.explorer.root, PathBuf::from("/root"));

    mem.fail_next(rune_vfs::OpKind::Resolve, std::io::ErrorKind::Other);

    let mut effects = Effects::default();
    let outcome = explorer_keys::handle_key(&mut app, key(KeyCode::Backspace), &mut effects);
    assert_eq!(outcome, KeyOutcome::Consumed);
    assert_eq!(
        effects.cmds.len(),
        1,
        "a resolve failure must not drop the ReadDir Cmd"
    );

    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);
    assert_eq!(app.explorer.root, PathBuf::from("/"));
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
                    path: PathBuf::from("/root/new.md"),
                    is_dir: false,
                },
                rune_vfs::DirEntry {
                    name: "a.md".to_string(),
                    path: PathBuf::from("/root/a.md"),
                    is_dir: false,
                },
                rune_vfs::DirEntry {
                    name: "b.md".to_string(),
                    path: PathBuf::from("/root/b.md"),
                    is_dir: false,
                },
                rune_vfs::DirEntry {
                    name: "sub".to_string(),
                    path: PathBuf::from("/root/sub"),
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
    let outcome = explorer_keys::handle_key(&mut app, key(KeyCode::Backspace), &mut effects);
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
                path: PathBuf::from("/stale/should-not-appear"),
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

    // `open_path` itself no longer moves focus (plan WP2 decision 6:
    // `switch_to` lost that write, and this function has no `Effects` sink
    // to run `App::set_focus` through) — this test's own re-activation
    // contract is `documents.len()`/`active` staying put, not a focus
    // assertion this change removes (plan gotcha 7).
    workspace::open_path(&mut app, Path::new("/root/a.md"));

    assert_eq!(app.documents.len(), before, "must not duplicate");
    assert_eq!(app.active, first_active);
}

/// The launch-mode gap the "Explorer visible on untitled launch" work left
/// open: showing the left column is not the same as filling it. A pathless
/// launch shows the column before any key is pressed, so the first listing
/// has to be requested from the bootstrap window rather than from the focus
/// chord — otherwise the user meets an empty box with a blank root row and
/// no way to guess that `^b` would populate it.
#[test]
fn an_untitled_app_requests_its_first_listing_without_any_keypress() {
    let mem = seeded_vfs();
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new_untitled(vfs);
    assert!(
        app.splits.left.is_shown(),
        "a pathless launch shows the left column"
    );
    assert!(app.explorer.entries.is_empty(), "nothing listed yet");

    let mut effects = Effects::default();
    explorer::ensure_loaded(&mut app, &mut effects);

    assert_eq!(effects.cmds.len(), 1, "the first listing must be requested");
    assert_eq!(effects.cmds[0].kind(), CmdKind::ReadDir);
    let msg = effects.cmds.remove(0).run().expect("ReadDir replies");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);
    assert!(
        !app.explorer.entries.is_empty(),
        "the pane the user can see must actually list something"
    );
}

/// A file-backed launch keeps the column collapsed, so there is nothing on
/// screen to fill and no listing should be requested — the load stays lazy
/// for exactly the launch mode that hides the pane.
#[test]
fn a_hidden_left_column_requests_no_listing() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    assert!(!app.splits.left.is_shown());

    let mut effects = Effects::default();
    explorer::ensure_loaded(&mut app, &mut effects);

    assert!(effects.cmds.is_empty(), "nothing visible, nothing to load");
}

/// `ensure_loaded` is reached from both the focus chord and the bootstrap
/// window, so it must not re-request a listing the Explorer already has —
/// otherwise every `^b` after the first would spend a filesystem round trip
/// re-reading a directory that is already on screen.
#[test]
fn ensure_loaded_is_a_no_op_once_the_explorer_has_entries() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    assert!(!app.explorer.entries.is_empty());

    let mut effects = Effects::default();
    explorer::ensure_loaded(&mut app, &mut effects);

    assert!(effects.cmds.is_empty(), "already listed; no reload");
}
