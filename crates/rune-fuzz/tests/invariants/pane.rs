//! Unit tests for `PANE-NO-BLEED` (the `UNDO-TOTAL` harness fix's own
//! pinned invariant, `src/invariant/pane.rs`): fires on a document
//! mutation landing behind a chrome-focused key; silent when the editor
//! is focused, when a modal owns the keyboard, when the active document
//! itself changed, and on any non-`Key` message.

use rune_fuzz::invariant::pane_no_bleed;
use rune_fuzz::step::MsgTag;
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::pane::Pane;

use crate::support::{base_active_id, base_ctx, base_snapshot, key, other_doc_id};

/// A `MsgTag::Key` `StepCtx` — `command: None` (an unbound chord), since
/// `PANE-NO-BLEED` only cares that the message IS a key, not which command
/// it resolved to.
fn key_ctx() -> rune_fuzz::step::StepCtx {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('x'), Mods::NONE),
        command: None,
    };
    ctx
}

#[test]
fn fires_on_an_edit_behind_an_explorer_focused_key() {
    let mut prev = base_snapshot("abc");
    prev.focus = Pane::Explorer;
    let mut next = base_snapshot("abcd");
    next.focus = Pane::Explorer;
    next.version = 2;
    next.journal_len = 1;
    let v = pane_no_bleed(&prev, &next, &key_ctx())
        .expect("a document mutation behind an Explorer-focused key must trip PANE-NO-BLEED");
    assert_eq!(v.id, "PANE-NO-BLEED");
}

#[test]
fn silent_when_focus_is_editor() {
    let prev = base_snapshot("abc"); // base_snapshot's default focus is Pane::Editor
    let mut next = base_snapshot("abcd");
    next.version = 2;
    next.journal_len = 1;
    assert_eq!(pane_no_bleed(&prev, &next, &key_ctx()), None);
}

#[test]
fn silent_when_a_modal_is_up() {
    let mut prev = base_snapshot("abc");
    prev.focus = Pane::Explorer;
    prev.modal_open = true;
    let mut next = base_snapshot("abcd");
    next.focus = Pane::Explorer;
    next.modal_open = true;
    next.version = 2;
    next.journal_len = 1;
    assert_eq!(pane_no_bleed(&prev, &next, &key_ctx()), None);
}

#[test]
fn silent_when_the_active_document_changed() {
    let mut prev = base_snapshot("abc");
    prev.focus = Pane::Explorer;
    prev.active = base_active_id();
    let mut next = base_snapshot("abcd");
    next.focus = Pane::Explorer;
    next.active = other_doc_id();
    next.version = 2;
    next.journal_len = 1;
    assert_eq!(pane_no_bleed(&prev, &next, &key_ctx()), None);
}

#[test]
fn silent_on_a_non_key_message() {
    let mut prev = base_snapshot("abc");
    prev.focus = Pane::Explorer;
    let mut next = base_snapshot("abcd");
    next.focus = Pane::Explorer;
    next.version = 2;
    next.journal_len = 1;
    let ctx = base_ctx(); // MsgTag::Resize
    assert_eq!(pane_no_bleed(&prev, &next, &ctx), None);
}
