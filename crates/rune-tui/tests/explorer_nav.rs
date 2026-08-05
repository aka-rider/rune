//! WP4 "Done when" tests: Explorer cursor movement and opening files/
//! directories, driven against a `Mem` vfs seeded with files and nested
//! directories — TODO.md's 500-line budget split of the original `explorer.rs`. The
//! `resolve` fallback, refresh/stale-reply handling, `open_path`
//! reactivation, and the lazy `ensure_loaded` load live in the sibling
//! `explorer_reload.rs`; both pull shared fixtures from `explorer_common`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod explorer_common;

use std::path::{Path, PathBuf};

use rune_tui::app;
use rune_tui::explorer_keys;
use rune_tui::keymap::{KeyCode, KeyOutcome};
use rune_tui::pane::Pane;
use rune_tui::runtime::{CmdKind, Effects};

use rune_vfs::Vfs;

use explorer_common::{app_with, key, load_explorer, seeded_vfs};

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
    // `load_explorer`'s `^b` now reveals the active document's own file
    // (the Enter/Escape rework's "land on the active file" contract), not
    // necessarily row 0 — pin a known starting row before probing the
    // clamp itself, which is what this test is actually about.
    app.explorer.nav.cursor = 0;
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
    // `load_explorer`'s `^b` now reveals the active document's own file,
    // not necessarily row 0 (see `up_and_down_clamp_at_the_list_bounds`'s
    // own note) — this test is about the ".." row specifically, so park the
    // cursor there explicitly.
    app.explorer.nav.cursor = 0;

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
