//! The GLOBAL `^w` (close-tab) and `^1`-`^0` (tab-switch) binding tests,
//! split out of `opentabs.rs` per TODO.md's §1.6 note: these were appended
//! to the Tabs-pane test file rather than split out when the global
//! bindings first landed. Pane-local Open Tabs rendering/switching and the
//! close guard's three resolutions stay in `opentabs.rs`; both files pull
//! shared fixtures from `opentabs_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod opentabs_common;

use rune_core::buffer::Buffer;
use rune_tui::app;
use rune_tui::commands::edit;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;

use opentabs_common::{app_with, key, open_second, seeded_vfs};

fn ctrl_w() -> KeyInput {
    key(
        KeyCode::Char('w'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    )
}

/// A `^`-modified digit chord, e.g. `ctrl(&'1')` for `^1`.
fn ctrl(c: char) -> KeyInput {
    key(
        KeyCode::Char(c),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    )
}

/// `^w`, end to end through the real four-stage pipeline with the Tabs
/// pane focused, now resolves at the GLOBAL pipeline stage (WP4's
/// `GlobalCommand::CloseFile`): it requests closing `app.active`, not
/// whichever row the Tabs cursor happens to sit on — arming the Guard for
/// a dirty active document exactly like calling `workspace::request_close`
/// directly.
#[test]
fn ctrl_w_on_the_tabs_pane_requests_closing_the_active_document() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');
    assert_eq!(
        app.active, second,
        "test setup: b.md is the active document"
    );
    // WP1: focus is gated on `LayoutMode` now — show the column first so
    // `Tabs` is actually painted and focusable.
    app.splits.left.show();
    app.set_focus_pane(Pane::Tabs, &mut Effects::default());
    app.tabs.nav.cursor = 0; // a.md's row — deliberately NOT the active document

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl_w()), &mut effects);

    assert!(
        app.modal.is_some(),
        "^w on the dirty active document must arm the Guard, regardless of the Tabs cursor"
    );
    assert!(app.documents.contains_key(&second));
}

/// `^w` from the EDITOR pane (WP4) closes the active clean document
/// straight away — no Guard, since there is nothing to lose.
#[test]
fn ctrl_w_from_editor_focus_closes_the_active_clean_document() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let first = app.active;
    let second = open_second(&mut app);
    assert_eq!(app.active, second);
    app.set_focus_pane(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl_w()), &mut effects);

    assert!(app.modal.is_none(), "a clean document closes immediately");
    assert!(!app.documents.contains_key(&second), "b.md must be closed");
    assert_eq!(app.active, first, "the sole remaining document takes over");
}

/// `^w` from the EDITOR pane on a DIRTY active document arms the Guard
/// instead of discarding it outright (WP4) — the same data-safety gate
/// `workspace::request_close` already gives every other close path.
#[test]
fn ctrl_w_from_editor_focus_on_a_dirty_document_arms_the_guard() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');
    assert!(app.doc(second).unwrap().is_dirty());
    app.set_focus_pane(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl_w()), &mut effects);

    match &app.modal {
        Some(rune_tui::banner::Modal::Guard(prompt)) => {
            assert_eq!(prompt.doc, second);
            assert_eq!(prompt.kind, rune_tui::banner::GuardKind::DirtyClose);
        }
        Some(_) => panic!("expected a DirtyClose Guard, got some other modal"),
        None => panic!("expected a DirtyClose Guard, got no modal"),
    }
    assert!(
        app.documents.contains_key(&second),
        "must not close before the Guard is resolved"
    );
}

/// `^1`, end to end through the real pipeline from the EDITOR pane (WP4),
/// jumps straight to the first tab.
#[test]
fn ctrl_1_switches_to_the_first_tab() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let first = app.active;
    open_second(&mut app);
    app.set_focus_pane(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl('1')), &mut effects);

    assert_eq!(app.active, app.tabs.order[0]);
    assert_eq!(app.active, first);
}

/// `^0` is the TENTH tab (WP4) — matching what the tab strip itself prints
/// for the first ten tabs (`(idx + 1) % 10`).
#[test]
fn ctrl_0_switches_to_the_tenth_tab() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    for i in 0..9 {
        app.open_document(Buffer::new(format!("doc {i}")));
    }
    assert_eq!(app.tabs.order.len(), 10);
    let tenth = app.tabs.order[9];
    let away = app.tabs.order[0];
    workspace::switch_to(&mut app, away); // away from the tenth
    app.set_focus_pane(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl('0')), &mut effects);

    assert_eq!(app.active, tenth);
    assert_eq!(app.active, app.tabs.order[9]);
}

/// The routing proof (WP4): `^1` fired from EXPLORER focus still switches
/// tabs. If `TabSwitch` were resolved by a pane-local table instead of
/// `GLOBAL_BINDINGS`, this would fail — the Explorer pane has no such
/// binding of its own.
#[test]
fn ctrl_1_from_explorer_focus_switches_tabs() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let first = app.active;
    open_second(&mut app);
    // WP1: focus is gated on `LayoutMode` now — show the column first so
    // `Explorer` is actually painted and focusable.
    app.splits.left.show();
    app.set_focus_pane(Pane::Explorer, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl('1')), &mut effects);

    assert_eq!(app.active, first, "^1 switched to the first tab");
    assert_eq!(app.active, app.tabs.order[0]);
}

/// A digit chord past the number of open tabs is a silent no-op (WP4) —
/// no panic, no change of `app.active`.
#[test]
fn an_out_of_range_tab_digit_is_a_no_op() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    open_second(&mut app);
    assert_eq!(app.tabs.order.len(), 2);
    let before = app.active;
    app.set_focus_pane(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl('9')), &mut effects);

    assert_eq!(
        app.active, before,
        "^9 with only 2 tabs open must be a no-op"
    );
}
