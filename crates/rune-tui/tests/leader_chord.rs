//! WP5.S11 "Done when" tests: the held-space leader end-to-end through the
//! real `app::update` — the same public seam every other integration test
//! uses (`tests/tui_edit.rs`, `tests/chrome.rs`). Each test assigns a
//! `FixedSpaceProbe` (plan decision 3's test double) so the chord fires
//! deterministically, with no real hardware involved.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::{Cursor, CursorSet};
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::keystate::FixedSpaceProbe;
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

fn press(app: &mut App, code: KeyCode) {
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Key(KeyInput {
            code,
            mods: Mods::NONE,
        }),
        &mut effects,
    );
}

fn press_sup(app: &mut App, code: KeyCode) {
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Key(KeyInput {
            code,
            mods: Mods {
                sup: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );
}

/// Case 1: space held + `x` -> the left column shows, focus Explorer, and
/// the typed space is gone from the buffer.
#[test]
fn space_held_and_x_opens_the_explorer_and_retracts_the_space() {
    let mut app = app_for("hello");
    app.space_probe = Box::new(FixedSpaceProbe(true));

    press(&mut app, KeyCode::Char(' '));
    assert_eq!(app.active_doc().buffer.content(), "hello ");

    press(&mut app, KeyCode::Char('x'));

    assert!(app.splits.left.is_shown());
    assert_eq!(app.focus, Pane::Explorer);
    assert_eq!(
        app.active_doc().buffer.content(),
        "hello",
        "the speculative space must be retracted"
    );
}

/// Case 2 ("typing must still work"): space NOT held + `x` typed after a
/// space -> the buffer contains `" x"` and focus is unchanged.
#[test]
fn space_not_held_then_x_just_types_both_characters() {
    let mut app = app_for("hello");
    app.space_probe = Box::new(FixedSpaceProbe(false));

    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('x'));

    assert_eq!(app.active_doc().buffer.content(), "hello x");
    assert_eq!(app.focus, Pane::Editor);
    assert!(!app.splits.left.is_shown());
}

/// Case 3: a lone space press -> the buffer gains exactly one `' '` and
/// `speculative_space` is armed.
#[test]
fn a_lone_space_press_types_one_space_and_arms_the_flag() {
    let mut app = app_for("hello");
    app.space_probe = Box::new(FixedSpaceProbe(false));

    press(&mut app, KeyCode::Char(' '));

    assert_eq!(app.active_doc().buffer.content(), "hello ");
    assert_eq!(app.speculative_space, Some(app.active));
}

/// Case 4: two space presses -> the buffer gains `"  "` — the first arming
/// is cleared and re-armed by the second press, never compounded.
#[test]
fn two_space_presses_type_two_spaces_and_never_compound_the_arming() {
    let mut app = app_for("hello");
    app.space_probe = Box::new(FixedSpaceProbe(false));

    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char(' '));

    assert_eq!(app.active_doc().buffer.content(), "hello  ");
    assert_eq!(app.speculative_space, Some(app.active));
}

/// Case 5: space held + `x` when the byte left of the caret is NOT a space
/// (the caret moved, or something else intervened) -> the chord still
/// fires, but `retract_space`'s own guard means no byte is deleted.
#[test]
fn space_held_and_x_fires_the_chord_without_deleting_when_the_left_byte_is_not_a_space() {
    let mut app = app_for("hello");
    app.space_probe = Box::new(FixedSpaceProbe(true));

    // No literal space was typed, so `speculative_space` is `None` — the
    // chord must still fire (space is physically down) but retract nothing.
    press(&mut app, KeyCode::Char('x'));

    assert!(app.splits.left.is_shown());
    assert_eq!(app.focus, Pane::Explorer);
    assert_eq!(app.active_doc().buffer.content(), "hello");
}

/// Case 6 (the WP4 data-loss guard, end to end): space held + `x` with an
/// active selection -> the chord fires and the selection survives intact.
#[test]
fn space_held_and_x_with_an_active_selection_leaves_the_selection_intact() {
    let mut app = app_for("hello world");
    app.space_probe = Box::new(FixedSpaceProbe(true));
    let id = app.active;
    // Two things make this bite, and both were wrong in an earlier version:
    //
    // 1. The caret must sit immediately right of a space (position 6, so
    //    `byte(5) == ' '`). At position 5 the byte to the left is 'o' and
    //    `retract_space`'s byte guard returns first, so the selection guard
    //    is never reached and the test passes even when it is deleted.
    // 2. `speculative_space` must actually be armed, or `retract_space` is
    //    never called at all and the assertion is vacuous.
    //
    // Verified by mutation: remove the selection guard in
    // `commands::edit::retract_space` and this test must fail.
    app.doc_mut(id).unwrap().cursors = CursorSet::new(0).map(|c| Cursor {
        anchor: 0,
        position: 6,
        ..c
    });
    app.speculative_space = Some(id);

    press(&mut app, KeyCode::Char('x'));

    assert!(app.splits.left.is_shown());
    assert_eq!(app.focus, Pane::Explorer);
    assert_eq!(
        app.active_doc().buffer.content(),
        "hello world",
        "the selected text must survive the chord"
    );
    let c = app.active_doc().cursors.primary();
    assert_eq!((c.anchor, c.position), (0, 6), "the selection must survive");
}

/// Case 7: `⌘X` while space is held is still `Command::Cut`, never a leader
/// completion — `Mods::NONE` on the leader table's entries means holding
/// `sup` can never match it.
#[test]
fn cmd_x_while_space_is_held_is_still_cut_not_a_leader_completion() {
    let mut app = app_for("hello world");
    app.space_probe = Box::new(FixedSpaceProbe(true));
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(0).map(|c| Cursor {
        anchor: 0,
        position: 5,
        ..c
    });

    press_sup(&mut app, KeyCode::Char('x'));

    assert!(
        !app.splits.left.is_shown(),
        "⌘X must never be mistaken for ␣X"
    );
    assert_eq!(app.focus, Pane::Editor);
    assert_eq!(
        app.active_doc().buffer.content(),
        " world",
        "⌘X must still cut the selection"
    );
}
