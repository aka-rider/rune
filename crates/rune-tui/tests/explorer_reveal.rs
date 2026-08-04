//! WP4 "Done when" tests for `explorer_reveal::reveal`: pointing the
//! Explorer at a given file, re-rooting when necessary, and landing the
//! cursor exactly on it. Shares the `explorer_common` fixtures with the
//! rest of the WP4 Explorer suite.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod explorer_common;

use std::path::{Path, PathBuf};

use rune_tui::explorer_reveal::reveal;
use rune_tui::runtime::{CmdKind, Effects};

use rune_vfs::Vfs;

use explorer_common::{app_with, load_explorer, seeded_vfs};

/// A reveal target whose parent is already the Explorer's current root
/// moves the cursor synchronously — no `ReadDir` `Cmd` is issued.
#[test]
fn reveal_within_the_current_root_moves_the_cursor_without_a_reload() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    assert_eq!(app.explorer.root, PathBuf::from("/root"));

    let mut effects = Effects::default();
    reveal(&mut app, Path::new("/root/b.md"), &mut effects);

    assert!(effects.cmds.is_empty(), "same-root reveal must not reload");
    assert_eq!(
        app.explorer.entries[app.explorer.nav.cursor].path,
        PathBuf::from("/root/b.md")
    );
}

/// A reveal target spelled with a `..` segment (or a `./` prefix) must
/// still land on the right entry — `reveal` resolves the whole path once
/// through `workspace::resolve` before comparing it against anything, so
/// an unresolved caller path (a symlink, a relative segment) can't
/// silently miss the entry match and look like a deleted file.
#[test]
fn reveal_to_a_path_with_a_dotdot_segment_lands_on_the_right_entry() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    assert_eq!(app.explorer.root, PathBuf::from("/root"));

    let mut effects = Effects::default();
    reveal(&mut app, Path::new("/root/sub/../b.md"), &mut effects);

    assert!(effects.cmds.is_empty(), "same-root reveal must not reload");
    assert_eq!(
        app.explorer.entries[app.explorer.nav.cursor].path,
        PathBuf::from("/root/b.md")
    );
}

/// A reveal target in a different directory re-roots the Explorer and
/// lands the cursor on it once the `DirLoaded` reply is delivered.
#[test]
fn reveal_in_a_different_directory_reroots_and_lands_on_it() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);
    assert_eq!(app.explorer.root, PathBuf::from("/root"));

    let mut effects = Effects::default();
    reveal(&mut app, Path::new("/root/sub/c.md"), &mut effects);

    assert_eq!(effects.cmds.len(), 1, "a re-root issues exactly one Cmd");
    assert_eq!(effects.cmds[0].kind(), CmdKind::ReadDir);
    assert!(app.explorer.loading);

    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects2 = Effects::default();
    rune_tui::app::update(&mut app, msg, &mut effects2);

    assert_eq!(app.explorer.root, PathBuf::from("/root/sub"));
    assert_eq!(
        app.explorer.entries[app.explorer.nav.cursor].path,
        PathBuf::from("/root/sub/c.md")
    );
}

/// A reveal target named with invalid UTF-8 must still land on the
/// byte-exact entry — matching on `DirEntry::path`, never the lossy
/// `DirEntry::name`.
#[test]
fn reveal_lands_on_a_non_utf8_named_file_by_byte_exact_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let mem = seeded_vfs();
    let raw_name = OsStr::from_bytes(b"caf\xE9.md");
    let raw_path = PathBuf::from("/root").join(raw_name);
    mem.save_atomic(&raw_path, b"non-utf8-named content")
        .expect("seed the non-UTF-8 named file");

    let mut app = app_with(&mem);
    load_explorer(&mut app);

    let mut effects = Effects::default();
    reveal(&mut app, &raw_path, &mut effects);

    assert!(
        effects.cmds.is_empty(),
        "the file's parent is already the current root"
    );
    assert_eq!(
        app.explorer.entries[app.explorer.nav.cursor].path, raw_path,
        "the cursor must land on the byte-exact path, not a lossy name match"
    );
}

/// A stale `DirLoaded` reply (an older generation, superseded by a second
/// `reveal` call before the first's Cmd landed) must not consume the
/// pending reveal meant for the later request.
#[test]
fn a_stale_dir_loaded_does_not_consume_a_pending_reveal() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/other/d.md"), b"d content")
        .expect("seed other/d.md");
    let mut app = app_with(&mem);
    load_explorer(&mut app);

    let mut effects = Effects::default();
    reveal(&mut app, Path::new("/root/sub/c.md"), &mut effects);
    assert_eq!(effects.cmds.len(), 1);
    let stale_cmd = effects.cmds.remove(0);

    // A second reveal to a DIFFERENT directory (e.g. the user moved on)
    // supersedes the first, bumping `request_generation` again before the
    // stale Cmd's reply is delivered.
    let mut effects2 = Effects::default();
    reveal(&mut app, Path::new("/root/other/d.md"), &mut effects2);
    assert_eq!(effects2.cmds.len(), 1);

    // The stale reply lands now, carrying the superseded generation.
    let stale_msg = stale_cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects3 = Effects::default();
    rune_tui::app::update(&mut app, stale_msg, &mut effects3);

    assert_eq!(
        app.explorer.root,
        PathBuf::from("/root"),
        "the stale reply must not overwrite the current root"
    );
    assert_eq!(
        app.explorer.pending_reveal,
        Some(PathBuf::from("/root/other/d.md")),
        "the pending reveal for the live request must survive the stale reply"
    );

    // The live reply lands and finally consumes the pending reveal.
    let live_cmd = effects2.cmds.remove(0);
    let live_msg = live_cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects4 = Effects::default();
    rune_tui::app::update(&mut app, live_msg, &mut effects4);

    assert_eq!(app.explorer.root, PathBuf::from("/root/other"));
    assert_eq!(
        app.explorer.entries[app.explorer.nav.cursor].path,
        PathBuf::from("/root/other/d.md")
    );
    assert_eq!(app.explorer.pending_reveal, None);
}

/// Revealing an entry far down a long listing scrolls it into view —
/// reusing `explorer::ensure_visible`'s existing scroll-follow logic.
#[test]
fn reveal_scrolls_a_far_down_entry_into_view() {
    let mem = seeded_vfs();
    for i in 0..60 {
        mem.save_atomic(
            Path::new(&format!("/root/z{i:02}.md")),
            format!("content {i}").as_bytes(),
        )
        .expect("seed a filler file");
    }
    let mut app = app_with(&mem);
    load_explorer(&mut app);

    let target = PathBuf::from("/root/z59.md");
    let target_idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.path == target)
        .expect("z59.md listed");
    assert!(
        !app.explorer
            .nav
            .window(app.explorer.entries.len(), 20)
            .contains(&target_idx),
        "the target starts out below the fold"
    );

    let mut effects = Effects::default();
    reveal(&mut app, &target, &mut effects);

    assert!(effects.cmds.is_empty(), "same-root reveal must not reload");
    assert_eq!(app.explorer.nav.cursor, target_idx);
    let visible_height = 20; // matches the fixture's `set_size(80, 23)` layout budget
    assert!(
        app.explorer
            .nav
            .window(app.explorer.entries.len(), visible_height)
            .contains(&target_idx),
        "reveal must scroll the target entry into the visible window"
    );
}
