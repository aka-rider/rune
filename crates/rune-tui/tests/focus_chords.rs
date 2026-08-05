//! End-to-end coverage for the Enter/Escape focus rework: the direct
//! modifier chords that replaced the held-space leader (`␣X` etc, deleted
//! along with `keystate.rs`) are unaffected here except for `^B`/`⌘B`,
//! which is no longer "always show" — it is the single left-column
//! toggle (`GlobalCommand::ToggleLeft`) — and `^E`/`⌘E`/`^K`/`⌘K`, which no
//! longer resolve to anything at all. `FocusTabs`/`FocusTitle` still
//! resolve directly off `GLOBAL_BINDINGS`; no physical key state is ever
//! consulted, so every printable keystroke with no modifier is
//! unconditionally text.
//!
//! Drives the real `app::update` — the same public seam every other
//! integration test uses (`tests/tui_edit.rs`, `tests/chrome.rs`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::{Cursor, CursorSet};
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::{Mem, Vfs};

fn app_for(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    // Caret at the END of the seeded content — every test types onto the
    // tail, mirroring how a user would arrive there.
    app.active_doc_mut().cursors = CursorSet::new(content.len());
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();
    app
}

/// A document with a real, seeded `file_path` — the precondition
/// `explorer_reveal::reveal` needs (`focus_title`/Help's own draft has none).
fn app_with_file() -> App {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"hello")
        .expect("seed a.md");
    mem.save_atomic(std::path::Path::new("/root/b.md"), b"other")
        .expect("seed b.md");
    let vfs: Arc<dyn Vfs + Send + Sync> = mem;
    let mut app = App::new(
        Buffer::new("hello"),
        Some(std::path::PathBuf::from("/root/a.md")),
        vfs,
        None,
    );
    app.active_doc_mut().cursors = CursorSet::new(5);
    app.active_doc_mut().viewport.set_size(80, 23);
    app.frame_width = 100;
    app.frame_height = 30;
    app.sync_view();
    app
}

fn press(app: &mut App, code: KeyCode, mods: Mods) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(KeyInput { code, mods }), &mut effects);
}

/// Like `press`, but also runs every `Cmd` the press queued and feeds its
/// reply back through `app::update` — what a real session's runtime loop
/// does between the key landing and the next frame. Needed wherever a
/// `reveal` re-roots the Explorer: the cursor only lands once the
/// `DirLoaded` reply the queued `ReadDir` `Cmd` produces is delivered.
fn press_and_settle(app: &mut App, code: KeyCode, mods: Mods) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(KeyInput { code, mods }), &mut effects);
    for cmd in effects.cmds {
        if let Some(msg) = cmd.run() {
            let mut effects2 = Effects::default();
            app::update(app, msg, &mut effects2);
        }
    }
}

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};
const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};
const NONE: Mods = Mods::NONE;

#[test]
fn ctrl_b_and_cmd_b_both_show_and_focus_the_explorer() {
    for mods in [CTRL, SUP] {
        let mut app = app_for("hello");
        press(&mut app, KeyCode::Char('b'), mods);
        assert!(app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Explorer);
    }
}

/// The single toggle: a visible, Explorer-focused column hides on the next
/// press and hands focus back to the Editor — `^B`/`⌘B` is never a dead key
/// in either direction.
#[test]
fn ctrl_b_and_cmd_b_both_hide_the_explorer_and_focus_the_editor() {
    for mods in [CTRL, SUP] {
        let mut app = app_for("hello");
        press(&mut app, KeyCode::Char('b'), CTRL);
        assert!(app.splits.left.is_shown());

        press(&mut app, KeyCode::Char('b'), mods);

        assert!(!app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Editor);
    }
}

/// Pressing the toggle twice returns to exactly the state it started in —
/// identity for both visibility and focus.
#[test]
fn ctrl_b_pressed_twice_is_identity() {
    let mut app = app_for("hello");
    let shown_before = app.splits.left.is_shown();
    let focus_before = app.focus();
    press(&mut app, KeyCode::Char('b'), CTRL);
    press(&mut app, KeyCode::Char('b'), CTRL);
    assert_eq!(app.splits.left.is_shown(), shown_before);
    assert_eq!(app.focus(), focus_before);
}

/// The show branch lands the cursor on the file currently open in the
/// editor, not merely wherever the Explorer was last rooted.
#[test]
fn ctrl_b_show_branch_reveals_the_active_documents_file() {
    let mut app = app_with_file();
    press_and_settle(&mut app, KeyCode::Char('b'), CTRL);
    assert!(app.splits.left.is_shown());
    assert_eq!(app.focus(), Pane::Explorer);
    let cursor_entry = &app.explorer.entries[app.explorer.nav.cursor];
    assert_eq!(cursor_entry.path, std::path::PathBuf::from("/root/a.md"));
}

/// A document with no `file_path` (a draft) still focuses the Explorer on
/// the show branch — just without repositioning onto a file that doesn't
/// exist.
#[test]
fn ctrl_b_show_branch_on_a_pathless_draft_focuses_without_repositioning() {
    let mut app = app_for("hello");
    assert!(app.active_doc().file_path.is_none());
    press(&mut app, KeyCode::Char('b'), CTRL);
    assert!(app.splits.left.is_shown());
    assert_eq!(app.focus(), Pane::Explorer);
}

#[test]
fn ctrl_t_and_cmd_t_both_show_and_focus_tabs() {
    for mods in [CTRL, SUP] {
        let mut app = app_for("hello");
        press(&mut app, KeyCode::Char('t'), mods);
        assert!(app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Tabs);
    }
}

/// `^R` focuses the title field for a rename. `⌘R` is deliberately NOT
/// bound here — `EDITOR_BINDINGS`' `Reload` command already claims `⌘R` for
/// re-decoding an image document, and `GLOBAL_BINDINGS` resolves before any
/// pane ever sees the key, so binding `⌘R` globally too would make Reload
/// permanently unreachable by keyboard.
#[test]
fn ctrl_r_focuses_the_title() {
    let mut app = app_for("hello");
    press(&mut app, KeyCode::Char('r'), CTRL);
    assert_eq!(app.focus(), Pane::Title);
}

/// `^K`/`⌘K` are deleted entirely: neither resolves to a `GlobalCommand`, so
/// each falls through to whichever pane owns focus. From the Editor with no
/// modifier bound to it, that means plain text insertion. `^E`/`⌘E` is NOT
/// in this list any more (plan WP1): those two chords now resolve to
/// `GlobalCommand::ToggleMessages` — see `tests/messages.rs` for their own
/// coverage.
#[test]
fn ctrl_k_no_longer_resolves_to_anything() {
    for (ch, mods) in [('k', CTRL), ('k', SUP)] {
        let mut app = app_for("hello");
        press(&mut app, KeyCode::Char(ch), mods);
        // Neither chord moves focus or the left column — both are unbound,
        // consumed by nothing.
        assert_eq!(app.focus(), Pane::Editor);
        assert!(!app.splits.left.is_shown());
    }
}

/// The must-not-regress case the whole rework exists for: typing prose
/// containing a bare space followed by one of the letters a chord table
/// claims elsewhere (`e`, `t`, `x`, `r`, `z`, `b`, `k`) must insert literal
/// text with NO focus change — there is no mechanism in this crate capable
/// of treating an unmodified keystroke as anything but text.
#[test]
fn typing_prose_with_former_leader_letters_inserts_literal_text_only() {
    for word in [" e", " t", " x", " r", " z", " b", " k"] {
        let mut app = app_for("hello");
        for ch in word.chars() {
            press(&mut app, KeyCode::Char(ch), NONE);
        }
        assert_eq!(app.active_doc().buffer.content(), format!("hello{word}"));
        assert_eq!(app.focus(), Pane::Editor);
        assert!(!app.splits.left.is_shown());
    }
}

/// `⌘X` (Cut) must never be affected by any focus-chord table — `Mods`
/// matching is exact, so a `sup`-only chord on `x` (unbound in
/// `GLOBAL_BINDINGS`) falls through to the editor's own `Cut` binding
/// untouched.
#[test]
fn cmd_x_is_still_cut_never_a_focus_chord() {
    let mut app = app_for("hello world");
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(0).map(|c| Cursor {
        anchor: 0,
        position: 5,
        ..c
    });

    press(&mut app, KeyCode::Char('x'), SUP);

    assert!(!app.splits.left.is_shown());
    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.active_doc().buffer.content(), " world");
}

/// Escape's cascade, step 1: multiple cursors collapse to the primary and
/// focus STAYS in the editor.
#[test]
fn escape_with_multiple_cursors_collapses_and_stays_in_the_editor() {
    let mut app = app_for("hello world");
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new_from(&[
        Cursor {
            position: 0,
            anchor: 0,
            desired_col: 0,
            id: 1,
        },
        Cursor {
            position: 6,
            anchor: 6,
            desired_col: 0,
            id: 2,
        },
    ]);

    press(&mut app, KeyCode::Escape, NONE);

    assert_eq!(app.focus(), Pane::Editor);
    assert!(!app.active_doc().cursors.is_multi());
    assert!(!app.splits.left.is_shown());
}

/// Escape's cascade, step 2: a single cursor's selection collapses and
/// focus STAYS in the editor.
#[test]
fn escape_with_a_selection_collapses_and_stays_in_the_editor() {
    let mut app = app_for("hello world");
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(0).map(|c| Cursor {
        anchor: 0,
        position: 5,
        ..c
    });

    press(&mut app, KeyCode::Escape, NONE);

    assert_eq!(app.focus(), Pane::Editor);
    assert!(!app.active_doc().cursors.primary().has_selection());
    assert!(!app.splits.left.is_shown());
}

/// Escape's cascade, step 3: with no cursor/selection left to collapse, a
/// further Escape leaves the editor for the Explorer, unfolding the left
/// column if it was collapsed.
#[test]
fn a_third_escape_leaves_the_editor_for_the_explorer() {
    let mut app = app_with_file();
    assert!(!app.splits.left.is_shown());

    press_and_settle(&mut app, KeyCode::Escape, NONE);

    assert!(app.splits.left.is_shown());
    assert_eq!(app.focus(), Pane::Explorer);
    let cursor_entry = &app.explorer.entries[app.explorer.nav.cursor];
    assert_eq!(cursor_entry.path, std::path::PathBuf::from("/root/a.md"));
}

/// Escape from the editor on a draft (no `file_path`) still focuses the
/// Explorer — just leaves the listing/cursor untouched.
#[test]
fn escape_on_a_pathless_draft_focuses_the_explorer_without_repositioning() {
    let mut app = app_for("hello");
    assert!(app.active_doc().file_path.is_none());

    press(&mut app, KeyCode::Escape, NONE);

    assert_eq!(app.focus(), Pane::Explorer);
}
