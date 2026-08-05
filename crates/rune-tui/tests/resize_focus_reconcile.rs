//! `Msg::Resize` can change `LayoutMode` underneath a focus that was settled
//! before the resize landed — narrowing the terminal while the Explorer
//! holds focus, for instance, can make the mode resolve to `EditorOnly`
//! while `app.focus()` still names `Pane::Explorer`. Key dispatch routes on
//! `app.focus()`, so an un-reconciled resize stalls the user in a pane
//! nobody can see. These tests drive the real `app::update` — the same
//! public seam `tests/focus_chords.rs` uses — and pin that a resize always
//! leaves focus on a pane the resolved `LayoutMode` still calls focusable.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::layout::{MIN_CENTER_W, MIN_LEFT_PANE_W};
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

fn app_for(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.sync_view();
    app
}

fn press(app: &mut App, code: KeyCode, mods: Mods) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(code_input(code, mods)), &mut effects);
}

fn code_input(code: KeyCode, mods: Mods) -> rune_tui::keymap::KeyInput {
    rune_tui::keymap::KeyInput { code, mods }
}

fn resize(app: &mut App, width: u16, height: u16) {
    let mut effects = Effects::default();
    app::update(app, Msg::Resize(width, height), &mut effects);
}

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

/// Roomy enough that the left column and both its sections always fit.
const ROOMY_WIDTH: u16 = 100;
const ROOMY_HEIGHT: u16 = 30;

/// Narrower than `MIN_LEFT_PANE_W + MIN_CENTER_W` — `layout::resolve_mode`
/// can no longer fit the left column at all, so the mode resolves to
/// `LayoutMode::EditorOnly`.
const NARROW_WIDTH: u16 = MIN_LEFT_PANE_W + MIN_CENTER_W - 1;

/// Focus the Explorer, then narrow the frame far enough that the mode
/// resolves to `EditorOnly`: focus must be reconciled onto the Editor, the
/// one pane that mode still paints.
#[test]
fn narrowing_the_frame_moves_focus_off_a_pane_that_stopped_being_painted() {
    let mut app = app_for("hello");
    resize(&mut app, ROOMY_WIDTH, ROOMY_HEIGHT);
    press(&mut app, KeyCode::Char('b'), CTRL);
    assert_eq!(app.focus(), Pane::Explorer);

    resize(&mut app, NARROW_WIDTH, ROOMY_HEIGHT);

    assert_eq!(app.focus(), Pane::Editor);
}

/// The converse: widening the frame back out must never silently move focus
/// off the Editor. A user who is in the editor stays in the editor.
#[test]
fn widening_the_frame_back_does_not_move_focus_off_the_editor() {
    let mut app = app_for("hello");
    resize(&mut app, NARROW_WIDTH, ROOMY_HEIGHT);
    assert_eq!(app.focus(), Pane::Editor);

    resize(&mut app, ROOMY_WIDTH, ROOMY_HEIGHT);

    assert_eq!(app.focus(), Pane::Editor);
}

/// A resize that leaves the mode unchanged must not disturb focus at all —
/// reconciliation only ever fires when the pane actually stopped being
/// painted.
#[test]
fn a_resize_that_does_not_change_the_mode_leaves_focus_untouched() {
    let mut app = app_for("hello");
    resize(&mut app, ROOMY_WIDTH, ROOMY_HEIGHT);
    press(&mut app, KeyCode::Char('b'), CTRL);
    assert_eq!(app.focus(), Pane::Explorer);

    resize(&mut app, ROOMY_WIDTH + 1, ROOMY_HEIGHT + 1);

    assert_eq!(app.focus(), Pane::Explorer);
}

/// The subtler case: a column that stays painted while its Explorer section
/// alone collapses (`LayoutMode::Split { explorer: false, tabs: true }`) —
/// not the whole column vanishing (`EditorOnly`).
#[test]
fn a_collapsed_explorer_section_within_a_still_shown_column_reconciles_too() {
    let mut app = app_for("hello");
    resize(&mut app, ROOMY_WIDTH, ROOMY_HEIGHT);
    press(&mut app, KeyCode::Char('b'), CTRL);
    assert_eq!(app.focus(), Pane::Explorer);

    // Tall enough for the column itself, too short for the Explorer's own
    // floor to fit alongside the tab rows' floor — see `split::Split::
    // allot`'s `(None, Some(_))` arm.
    resize(&mut app, ROOMY_WIDTH, 6);

    assert_eq!(
        app.layout_mode(),
        rune_tui::focus::LayoutMode::Split {
            explorer: false,
            tabs: true
        }
    );
    assert_eq!(app.focus(), Pane::Editor);
}
