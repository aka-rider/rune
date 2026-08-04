//! End-to-end coverage for the direct modifier chords that replaced the
//! held-space leader (`␣X` etc, deleted along with `keystate.rs`): a
//! terminal cannot report the spacebar's physical state in-band, so a
//! prefix chord could never be told apart from plain text, and it
//! intermittently typed literal characters into the buffer instead of
//! moving focus. `FocusExplorer`/`FocusEditor`/`FocusTabs`/`FocusTitle`/
//! `CollapseLeft` now each resolve directly off `GLOBAL_BINDINGS` — no
//! physical key state is ever consulted, so every printable keystroke with
//! no modifier is unconditionally text.
//!
//! Drives the real `app::update` — the same public seam every other
//! integration test uses (`tests/tui_edit.rs`, `tests/chrome.rs`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

fn app_for(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    // Caret at the END of the seeded content — every test types onto the
    // tail, mirroring how a user would arrive there.
    app.active_doc_mut().cursors = CursorSet::new(content.len());
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();
    app
}

fn press(app: &mut App, code: KeyCode, mods: Mods) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(KeyInput { code, mods }), &mut effects);
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

#[test]
fn ctrl_b_and_cmd_b_both_show_and_focus_the_explorer() {
    for mods in [CTRL, SUP] {
        let mut app = app_for("hello");
        press(&mut app, KeyCode::Char('b'), mods);
        assert!(app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Explorer);
    }
}

#[test]
fn ctrl_e_and_cmd_e_both_focus_the_editor() {
    for mods in [CTRL, SUP] {
        let mut app = app_for("hello");
        press(&mut app, KeyCode::Char('b'), CTRL);
        assert_eq!(app.focus(), Pane::Explorer);
        press(&mut app, KeyCode::Char('e'), mods);
        assert_eq!(app.focus(), Pane::Editor);
    }
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

#[test]
fn ctrl_k_and_cmd_k_both_collapse_the_left_column_and_return_focus_to_the_editor() {
    for mods in [CTRL, SUP] {
        let mut app = app_for("hello");
        press(&mut app, KeyCode::Char('b'), CTRL);
        assert!(app.splits.left.is_shown());

        press(&mut app, KeyCode::Char('k'), mods);

        assert!(!app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Editor);
    }
}

/// Pressing the same focus chord twice must never toggle anything back off
/// — a regression the old `␣X` leader guarded against and the direct chord
/// inherits unchanged.
#[test]
fn ctrl_b_pressed_twice_leaves_the_explorer_shown_and_focused() {
    let mut app = app_for("hello");
    press(&mut app, KeyCode::Char('b'), CTRL);
    press(&mut app, KeyCode::Char('b'), CTRL);
    assert!(app.splits.left.is_shown());
    assert_eq!(app.focus(), Pane::Explorer);
}

/// The must-not-regress case the whole rework exists for: typing prose
/// containing a bare space followed by one of the letters the old leader
/// used to claim (`e`, `t`, `x`, `r`, `z`, plus `b`/`k` for the new chords)
/// must insert literal text with NO focus change and no character ever
/// speculatively appearing then vanishing — there is no longer any
/// mechanism in this crate capable of doing that at all.
#[test]
fn typing_prose_with_former_leader_letters_inserts_literal_text_only() {
    for word in [" e", " t", " x", " r", " z", " b", " k"] {
        let mut app = app_for("hello");
        for ch in word.chars() {
            press(&mut app, KeyCode::Char(ch), Mods::NONE);
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
    app.doc_mut(id).unwrap().cursors = CursorSet::new(0).map(|c| rune_core::cursor::Cursor {
        anchor: 0,
        position: 5,
        ..c
    });

    press(&mut app, KeyCode::Char('x'), SUP);

    assert!(!app.splits.left.is_shown());
    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.active_doc().buffer.content(), " world");
}
