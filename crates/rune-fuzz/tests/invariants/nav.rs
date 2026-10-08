//! Unit tests for `NAV-PROGRESS`. Same controlled-experiment pattern as
//! every other file here: a hand-built stuck-caret pair that must fire, and
//! companions of the same shape that must stay silent.

use rune_fuzz::invariant::nav_progress;
use rune_fuzz::snapshot::Snapshot;
use rune_fuzz::step::{MsgTag, StepCtx};
use rune_tui::document::ReadOnly;
use rune_tui::focus::FocusTarget;
use rune_tui::keymap::{Command, Extend, KeyCode, Mods, Motion};

use crate::support::{base_ctx, base_snapshot, collapsed_cursor, key};

const CONTENT: &str = "ab\ncd\nef\n";

fn motion_ctx(motion: Motion, extend: Extend) -> StepCtx {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Down, Mods::NONE),
        command: Some(Command::Motion(motion, extend)),
    };
    ctx
}

fn down() -> StepCtx {
    motion_ctx(Motion::LineDown, Extend::No)
}

fn up() -> StepCtx {
    motion_ctx(Motion::LineUp, Extend::No)
}

fn with_caret(position: usize) -> Snapshot {
    let mut snap = base_snapshot(CONTENT);
    snap.cursors = vec![collapsed_cursor(1, position)];
    snap
}

#[test]
fn nav_progress_detects_a_down_that_leaves_the_caret_mid_buffer() {
    let snap = with_caret(4);
    let v = nav_progress(&snap, &snap, &down()).expect("caret did not move");
    assert_eq!(v.id, "NAV-PROGRESS");
}

#[test]
fn nav_progress_detects_an_up_that_leaves_the_caret_below_the_first_line() {
    let snap = with_caret(4);
    let v = nav_progress(&snap, &snap, &up()).expect("caret did not move");
    assert_eq!(v.id, "NAV-PROGRESS");
}

#[test]
fn nav_progress_accepts_a_caret_that_moved() {
    assert_eq!(nav_progress(&with_caret(1), &with_caret(4), &down()), None);
}

#[test]
fn nav_progress_accepts_down_at_the_buffer_end() {
    let snap = with_caret(CONTENT.len());
    assert_eq!(nav_progress(&snap, &snap, &down()), None);
}

#[test]
fn nav_progress_accepts_up_on_the_first_line_which_hands_focus_to_the_title() {
    let snap = with_caret(1);
    assert_eq!(nav_progress(&snap, &snap, &up()), None);
}

#[test]
fn nav_progress_ignores_the_extending_variant() {
    let snap = with_caret(4);
    let ctx = motion_ctx(Motion::LineDown, Extend::Yes);
    assert_eq!(nav_progress(&snap, &snap, &ctx), None);
}

#[test]
fn nav_progress_ignores_multi_cursor_sets() {
    let mut snap = with_caret(4);
    snap.cursors.push(collapsed_cursor(2, 7));
    assert_eq!(nav_progress(&snap, &snap, &down()), None);
}

#[test]
fn nav_progress_ignores_keys_that_are_not_those_motions() {
    let snap = with_caret(4);
    assert_eq!(nav_progress(&snap, &snap, &base_ctx()), None);
    let ctx = motion_ctx(Motion::CharRight, Extend::No);
    assert_eq!(nav_progress(&snap, &snap, &ctx), None);
}

#[test]
fn nav_progress_ignores_a_key_the_editor_did_not_receive() {
    let mut overlay = with_caret(4);
    overlay.focus_target = FocusTarget::Find;
    assert_eq!(nav_progress(&overlay, &overlay, &down()), None);

    let mut guarded = with_caret(4);
    guarded.modal_open = true;
    assert_eq!(nav_progress(&guarded, &guarded, &down()), None);

    let mut reading = with_caret(4);
    reading.read_only = ReadOnly::Reading;
    assert_eq!(nav_progress(&reading, &reading, &down()), None);
}
