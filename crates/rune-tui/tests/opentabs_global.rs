//! The GLOBAL `^w` (close-tab) and `^1`-`^0` (tab-switch) binding tests,
//! split out of `opentabs.rs` per TODO.md's 500-line budget note: these were appended
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

use opentabs_common::{key, open_second, open_seeded};

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
/// pane focused, resolves at the GLOBAL pipeline stage (
/// `GlobalCommand::CloseFile`): it requests closing `app.active`, not
/// whichever row the Tabs cursor happens to sit on — arming the Guard for
/// a dirty active document exactly like calling `workspace::request_close`
/// directly.
#[test]
fn ctrl_w_on_the_tabs_pane_requests_closing_the_active_document() {
    let mut session = open_seeded();
    let second = open_second(&mut session);
    edit::insert_char(session.app_mut(), second, '!');
    assert_eq!(
        session.app().active,
        second,
        "test setup: b.md is the active document"
    );
    // Focus is gated on `LayoutMode` — show the column first so `Tabs`
    // is actually painted and focusable.
    session.app_mut().splits.left.show();
    session
        .app_mut()
        .set_focus_pane(Pane::Tabs, &mut Effects::default());
    session.app_mut().tabs.nav.cursor = 0; // a.md's row — deliberately NOT the active document

    let mut effects = Effects::default();
    app::update(session.app_mut(), Msg::Key(ctrl_w()), &mut effects);

    assert!(
        session.app().guard.is_some(),
        "^w on the dirty active document must arm the Guard, regardless of the Tabs cursor"
    );
    assert!(session.app().documents.contains_key(&second));
}

/// `^w` from the EDITOR pane closes the active clean document
/// straight away — no Guard, since there is nothing to lose.
#[test]
fn ctrl_w_from_editor_focus_closes_the_active_clean_document() {
    let mut session = open_seeded();
    let first = session.app().active;
    let second = open_second(&mut session);
    assert_eq!(session.app().active, second);
    session
        .app_mut()
        .set_focus_pane(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(session.app_mut(), Msg::Key(ctrl_w()), &mut effects);

    assert!(
        session.app().guard.is_none(),
        "a clean document closes immediately"
    );
    assert!(
        !session.app().documents.contains_key(&second),
        "b.md must be closed"
    );
    assert_eq!(
        session.app().active,
        first,
        "the sole remaining document takes over"
    );
}

/// `^w` from the EDITOR pane on a DIRTY active document arms the Guard
/// instead of discarding it outright — the same data-safety gate
/// `workspace::request_close` already gives every other close path.
#[test]
fn ctrl_w_from_editor_focus_on_a_dirty_document_arms_the_guard() {
    let mut session = open_seeded();
    let second = open_second(&mut session);
    edit::insert_char(session.app_mut(), second, '!');
    assert!(session.app().doc(second).unwrap().is_dirty());
    session
        .app_mut()
        .set_focus_pane(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(session.app_mut(), Msg::Key(ctrl_w()), &mut effects);

    match &session.app().guard {
        Some(prompt) => {
            assert_eq!(prompt.doc, second);
            assert_eq!(prompt.kind, rune_tui::guard::GuardKind::DirtyClose);
        }
        None => panic!("expected a DirtyClose Guard, got no Guard"),
    }
    assert!(
        session.app().documents.contains_key(&second),
        "must not close before the Guard is resolved"
    );
}

/// `^1`, end to end through the real pipeline from the EDITOR pane,
/// jumps straight to the first tab.
#[test]
fn ctrl_1_switches_to_the_first_tab() {
    let mut session = open_seeded();
    let first = session.app().active;
    open_second(&mut session);
    session
        .app_mut()
        .set_focus_pane(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(session.app_mut(), Msg::Key(ctrl('1')), &mut effects);

    assert_eq!(session.app().active, session.app().documents.order()[0]);
    assert_eq!(session.app().active, first);
}

/// `^0` is the TENTH tab — matching what the tab strip itself prints
/// for the first ten tabs (`(idx + 1) % 10`).
#[test]
fn ctrl_0_switches_to_the_tenth_tab() {
    let mut session = open_seeded();
    for i in 0..9 {
        session
            .app_mut()
            .open_document(Buffer::new(format!("doc {i}")));
    }
    assert_eq!(session.app().documents.order().len(), 10);
    let tenth = session.app().documents.order()[9];
    let away = session.app().documents.order()[0];
    workspace::switch_to(session.app_mut(), away); // away from the tenth
    session
        .app_mut()
        .set_focus_pane(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(session.app_mut(), Msg::Key(ctrl('0')), &mut effects);

    assert_eq!(session.app().active, tenth);
    assert_eq!(session.app().active, session.app().documents.order()[9]);
}

/// The routing proof: `^1` fired from EXPLORER focus still switches
/// tabs. If `TabSwitch` were resolved by a pane-local table instead of
/// `GLOBAL_BINDINGS`, this would fail — the Explorer pane has no such
/// binding of its own.
#[test]
fn ctrl_1_from_explorer_focus_switches_tabs() {
    let mut session = open_seeded();
    let first = session.app().active;
    open_second(&mut session);
    // Focus is gated on `LayoutMode` — show the column first so
    // `Explorer` is actually painted and focusable.
    session.app_mut().splits.left.show();
    session
        .app_mut()
        .set_focus_pane(Pane::Explorer, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(session.app_mut(), Msg::Key(ctrl('1')), &mut effects);

    assert_eq!(session.app().active, first, "^1 switched to the first tab");
    assert_eq!(session.app().active, session.app().documents.order()[0]);
}

/// A digit chord past the number of open tabs is a silent no-op —
/// no panic, no change of `app.active`.
#[test]
fn an_out_of_range_tab_digit_is_a_no_op() {
    let mut session = open_seeded();
    open_second(&mut session);
    assert_eq!(session.app().documents.order().len(), 2);
    let before = session.app().active;
    session
        .app_mut()
        .set_focus_pane(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    app::update(session.app_mut(), Msg::Key(ctrl('9')), &mut effects);

    assert_eq!(
        session.app().active,
        before,
        "^9 with only 2 tabs open must be a no-op"
    );
}
