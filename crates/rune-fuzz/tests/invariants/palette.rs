//! Unit tests for the command-palette invariants: `PALETTE-FOCUS-STABLE`,
//! `PALETTE-GUARD`. Same controlled-experiment pattern as every other file
//! here — one hand-built BAD `Snapshot`/pair per checker asserting it fires
//! with the right id, one well-formed companion of the same shape asserting
//! `None`.

use rune_fuzz::invariant::{palette_focus_stable, palette_guard};
use rune_fuzz::step::MsgTag;
use rune_tui::focus::FocusTarget;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::pane::Pane;

use crate::support::{base_active_id, base_ctx, base_snapshot, key};

fn key_ctx() -> rune_fuzz::step::StepCtx {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Escape, Mods::NONE),
        command: None,
    };
    ctx
}

#[test]
fn palette_focus_stable_ignores_opening_the_palette_from_title_focus() {
    let prev = base_snapshot("abc"); // focus_target: Editor, focus: Editor
    let mut next = base_snapshot("abc");
    next.focus_target = FocusTarget::Palette;
    next.focus = Pane::Editor;
    assert_eq!(palette_focus_stable(&prev, &next, &key_ctx()), None);
}

#[test]
fn palette_focus_stable_ignores_closing_the_palette() {
    let mut prev = base_snapshot("abc");
    prev.focus_target = FocusTarget::Palette;
    let next = base_snapshot("abc"); // focus_target: Editor again
    assert_eq!(palette_focus_stable(&prev, &next, &key_ctx()), None);
}

#[test]
fn palette_focus_stable_accepts_unchanged_focus_while_the_palette_stays_open() {
    let mut prev = base_snapshot("abc");
    prev.focus_target = FocusTarget::Palette;
    let mut next = base_snapshot("abc");
    next.focus_target = FocusTarget::Palette;
    assert_eq!(palette_focus_stable(&prev, &next, &key_ctx()), None);
}

#[test]
fn palette_focus_stable_ignores_a_resize_driven_layout_reconcile() {
    let mut prev = base_snapshot("abc");
    prev.focus_target = FocusTarget::Palette;
    let mut next = base_snapshot("abc");
    next.focus_target = FocusTarget::Palette;
    next.focus = Pane::Explorer; // a resize can legitimately reconcile this
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Resize(1, 6);
    assert_eq!(palette_focus_stable(&prev, &next, &ctx), None);
}

#[test]
fn palette_focus_stable_detects_focus_moving_on_a_key_while_the_palette_stays_open() {
    let mut prev = base_snapshot("abc");
    prev.focus_target = FocusTarget::Palette;
    let mut next = base_snapshot("abc");
    next.focus_target = FocusTarget::Palette;
    next.focus = Pane::Tabs;
    let v = palette_focus_stable(&prev, &next, &key_ctx())
        .expect("focus moved to Tabs on a key while the palette stayed open");
    assert_eq!(v.id, "PALETTE-FOCUS-STABLE");
}

#[test]
fn palette_guard_accepts_no_guard_while_the_palette_is_open() {
    let mut next = base_snapshot("abc");
    next.focus_target = FocusTarget::Palette;
    assert_eq!(palette_guard(&next), None);
}

#[test]
fn palette_guard_detects_a_guard_raised_while_the_palette_is_still_open() {
    let mut next = base_snapshot("abc");
    next.focus_target = FocusTarget::Palette;
    next.guard = Some((base_active_id(), GuardKind::DirtyQuit));
    let v = palette_guard(&next).expect("a Guard came up but the palette stayed open");
    assert_eq!(v.id, "PALETTE-GUARD");
}
