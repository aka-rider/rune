//! Explorer type-to-search "Done when" integration tests, driven end-to-end
//! through `app::update` (not `explorer_keys::handle_key` directly, unlike
//! the sibling unit tests in `explorer_search.rs` itself) — these exercise
//! the whole four-stage key pipeline plus the real `^b` load, matching how
//! a user's keystrokes actually reach the Explorer. Shared fixtures come
//! from `explorer_common`, the same ones `explorer_nav.rs`/
//! `explorer_reload.rs` already use.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod explorer_common;

use rune_tui::app;
use rune_tui::keymap::KeyCode;
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};

use explorer_common::{app_with, key, load_explorer, seeded_vfs};

/// End-to-end: `^b` loads the Explorer, typing "a" jumps the cursor to
/// "a.md", and Enter opens it — landing focus on the Editor, exactly the
/// same as opening any other selected entry.
#[test]
fn ctrl_b_type_then_enter_opens_the_searched_file_and_focuses_the_editor() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(key(KeyCode::Char('a'))), &mut effects);

    assert_eq!(app.explorer.search.as_deref(), Some("a"));
    let cursor_name = app.explorer.entries[app.explorer.nav.cursor].name.clone();
    assert_eq!(cursor_name, "a.md", "typing 'a' must jump to a.md");

    let mut effects2 = Effects::default();
    app::update(&mut app, Msg::Key(key(KeyCode::Enter)), &mut effects2);

    assert_eq!(
        app.focus(),
        Pane::Editor,
        "Enter must open the file and focus the editor"
    );
    assert_eq!(
        app.explorer.search, None,
        "opening a file must clear the search"
    );
}

/// Navigating into a directory (Enter on "sub") clears the search — the
/// `handle_dir_loaded` chokepoint, since the entries a query matched
/// against no longer describe what's on screen once a new listing lands.
#[test]
fn navigating_into_a_directory_clears_the_search() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(key(KeyCode::Char('s'))), &mut effects);
    assert_eq!(app.explorer.search.as_deref(), Some("s"));
    let cursor_name = app.explorer.entries[app.explorer.nav.cursor].name.clone();
    assert_eq!(
        cursor_name, "sub",
        "typing 's' must jump to the 'sub' directory"
    );

    // Enter on "sub" issues a ReadDir Cmd (a directory, not a file) —
    // deliver its reply the same way `load_explorer` does for the initial
    // load, then confirm the search that led here didn't survive it.
    let mut effects2 = Effects::default();
    app::update(&mut app, Msg::Key(key(KeyCode::Enter)), &mut effects2);
    assert_eq!(
        effects2.cmds.len(),
        1,
        "opening a dir must enqueue one ReadDir Cmd"
    );
    let cmd = effects2.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects3 = Effects::default();
    app::update(&mut app, msg, &mut effects3);

    assert_eq!(
        app.explorer.root,
        std::path::PathBuf::from("/root/sub"),
        "must have navigated into sub"
    );
    assert_eq!(
        app.explorer.search, None,
        "a directory reload must clear the search that led into it"
    );
}

/// Focusing away from the Explorer (here, to the Editor via `^b`'s hide
/// branch — `^e` is deleted along with the rest of the Enter/Escape rework)
/// clears a live search — the design's "leaving the Explorer -> search
/// cleared" rule, enforced by `app::set_focus`'s own blur chokepoint.
#[test]
fn focusing_away_from_the_explorer_clears_the_search() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    load_explorer(&mut app);

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(key(KeyCode::Char('a'))), &mut effects);
    assert!(app.explorer.search.is_some());

    let ctrl_b = rune_tui::keymap::KeyInput {
        code: KeyCode::Char('b'),
        mods: rune_tui::keymap::Mods {
            ctrl: true,
            ..rune_tui::keymap::Mods::NONE
        },
    };
    let mut effects2 = Effects::default();
    app::update(&mut app, Msg::Key(ctrl_b), &mut effects2);

    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(
        app.explorer.search, None,
        "leaving the Explorer must clear a live search"
    );
}
