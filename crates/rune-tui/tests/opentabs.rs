//! Tests for Open Tabs rendering/switching, driven against a `Mem` vfs
//! seeded with two files. The close guard's three resolutions
//! (`[S]ave`, `[D]iscard`, `Esc`) live in the sibling `opentabs_guard.rs`
//! (500-line budget); the GLOBAL `^w`/`^1`-`^0` binding tests live in
//! `opentabs_global.rs`; all three pull shared fixtures from
//! `opentabs_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod opentabs_common;

use rune_tui::commands::edit;
use rune_tui::keymap::KeyCode;
use rune_tui::pane::Pane;
use rune_tui::runtime::Effects;
use rune_tui::{opentabs, workspace};

use opentabs_common::{frame_text, open_second, open_seeded, plain};

/// Opening two documents populates `documents.order()`, and both render with their
/// digit shortcut and name below the `Open` divider row.
#[test]
fn tabs_render_both_open_documents_with_digit_shortcuts() {
    let mut session = open_seeded();
    open_second(&mut session);
    assert_eq!(session.app().documents.order().len(), 2);

    session.app_mut().splits.left.show();
    session
        .app_mut()
        .set_focus_pane(Pane::Tabs, &mut Effects::default());
    session.app_mut().sync_view();

    let text = frame_text(&mut session);
    assert!(
        text.contains("1:"),
        "expected the first tab's shortcut '1:' in:\n{text}"
    );
    assert!(
        text.contains("2:"),
        "expected the second tab's shortcut '2:' in:\n{text}"
    );
    assert!(text.contains("a.md"));
    assert!(text.contains("b.md"));
}

/// The Open Tabs section is introduced by a divider ROW inside the left
/// column's single border — there is no separate titled block, so the tab
/// rows follow immediately underneath it.
#[test]
fn the_open_divider_row_precedes_the_tab_rows() {
    let mut session = open_seeded();
    open_second(&mut session);
    session.app_mut().splits.left.show();
    session
        .app_mut()
        .set_focus_pane(Pane::Tabs, &mut Effects::default());
    session.app_mut().sync_view();

    let rows = session.grid(opentabs_common::WIDTH, opentabs_common::HEIGHT);
    let divider = rows
        .iter()
        .position(|r| r.contains(" Open "))
        .unwrap_or_else(|| panic!("expected an Open divider row in:\n{}", rows.join("\n")));

    assert!(
        rows[divider].contains('\u{2500}'),
        "the divider row must be filled out with `\u{2500}`:\n{}",
        rows[divider]
    );
    assert!(
        rows[divider + 1].contains("a.md"),
        "the first tab row must sit directly under the divider:\n{}",
        rows[divider + 1]
    );
    assert!(
        rows[divider + 2].contains("b.md"),
        "the second tab row follows it:\n{}",
        rows[divider + 2]
    );
}

/// Enter on a cursor row switches the active document —
/// driven through `opentabs::handle_key` directly, the same style
/// `tests/explorer.rs` already uses for its own pane-local assertions.
#[test]
fn enter_switches_the_active_document() {
    let mut session = open_seeded();
    let first = session.app().active;
    let second = open_second(&mut session);
    session
        .app_mut()
        .set_focus_pane(Pane::Editor, &mut Effects::default());
    workspace::switch_to(session.app_mut(), first); // back to a.md, cursor -> index 0

    session.app_mut().tabs.nav.cursor = 1; // b.md's row
    let outcome = opentabs::handle_key(
        session.app_mut(),
        plain(KeyCode::Enter),
        &mut Effects::default(),
    );

    assert_eq!(outcome, rune_tui::keymap::KeyOutcome::Consumed);
    assert_eq!(session.app().active, second);
    assert_eq!(session.app().focus(), Pane::Editor);
}

/// A dirty document's tab shows the `x` dirty marker; a clean one shows a
/// blank in its place. The row shape pins the fixed marker
/// columns: pin, dirty, sync (blank here), separator, name.
#[test]
fn dirty_dot_appears_after_an_edit_to_the_active_document() {
    let mut session = open_seeded();
    let second = open_second(&mut session);
    session.app_mut().splits.left.show();
    session.app_mut().sync_view();
    assert!(
        !frame_text(&mut session).contains(" x "),
        "test setup: nothing should be dirty yet"
    );

    edit::insert_char(session.app_mut(), second, '!');
    session.app_mut().sync_view();

    assert!(session.app().doc(second).unwrap().is_dirty());
    let text = frame_text(&mut session);
    assert!(
        text.contains(" x  b.md"),
        "expected the dirty marker in b.md's tab row:\n{text}"
    );
}

/// A background tab whose document diverged on disk shows the `⇄` marker
/// in its own fixed column — per-doc state, visible even while a different
/// (clean) document is active, so the footer shows no marker of its own.
#[test]
fn diverged_background_doc_tab_shows_the_sync_marker() {
    let mut session = open_seeded();
    let first = session.app().active;
    let second = open_second(&mut session);
    workspace::switch_to(session.app_mut(), first);
    session.app_mut().splits.left.show();
    session.app_mut().sync_view();
    assert!(
        !frame_text(&mut session).contains('\u{21c4}'),
        "test setup: nothing should be diverged yet"
    );

    session.app_mut().doc_mut(second).unwrap().last_sync = Some(rune_db::SyncKind::Diverged);
    session.app_mut().sync_view();

    let text = frame_text(&mut session);
    assert!(
        text.contains("\u{21c4} b.md"),
        "expected the sync marker in b.md's tab row:\n{text}"
    );
    assert!(
        !text.contains("\u{21c4} a.md"),
        "the clean a.md row must not carry the marker:\n{text}"
    );
}
