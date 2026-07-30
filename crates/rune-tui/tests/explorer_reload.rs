//! WP4/WP13 "Done when" tests: the Explorer's `resolve` fallback, refresh
//! and stale-reply handling, `workspace::open_path` reactivation, and the
//! lazy `ensure_loaded` load — TODO.md's §1.6 split of the original
//! `explorer.rs`. Cursor movement and opening files/directories live in
//! the sibling `explorer_nav.rs`; both pull shared fixtures from
//! `explorer_common`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod explorer_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_tui::app::{self, App};
use rune_tui::keymap::KeyCode;
use rune_tui::keymap::KeyOutcome;
use rune_tui::pane::Pane;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::{explorer, explorer_keys, workspace};
use rune_vfs::Vfs;

use explorer_common::{app_with, key, load_explorer, seeded_vfs};

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
    app.focus = Pane::Explorer;

    workspace::open_path(&mut app, Path::new("/root/a.md"));

    assert_eq!(app.documents.len(), before, "must not duplicate");
    assert_eq!(app.active, first_active);
    assert_eq!(app.focus, Pane::Editor);
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
