//! Unit tests for `PANE-NO-BLEED` (the `UNDO-TOTAL` harness fix's own
//! pinned invariant, `src/invariant/pane.rs`): fires on a document
//! mutation landing behind a chrome-focused key; silent when the editor
//! is focused, when a modal owns the keyboard, when the active document
//! itself changed, and on any non-`Key` message.

use rune_core::undo::EditKind;
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

#[test]
fn silent_when_a_save_strips_trailing_whitespace_behind_a_title_focused_key() {
    let mut prev = base_snapshot("# \n\n\n");
    prev.focus = Pane::Title;
    prev.journal_pos = 5;
    prev.journal_len = 5;
    let mut next = base_snapshot("#\n\n\n");
    next.focus = Pane::Title;
    next.version = 2;
    next.journal_pos = 6;
    next.journal_len = 6;
    next.newest_applied_edit_kind = Some(EditKind::StripTrailingWhitespace);
    assert_eq!(pane_no_bleed(&prev, &next, &key_ctx()), None);
}

#[test]
fn fires_on_an_ordinary_insert_behind_a_title_focused_key() {
    let mut prev = base_snapshot("abc");
    prev.focus = Pane::Title;
    prev.journal_pos = 5;
    prev.journal_len = 5;
    let mut next = base_snapshot("abcd");
    next.focus = Pane::Title;
    next.version = 2;
    next.journal_pos = 6;
    next.journal_len = 6;
    next.newest_applied_edit_kind = Some(EditKind::Insert);
    let v = pane_no_bleed(&prev, &next, &key_ctx())
        .expect("a plain insert behind a Title-focused key must still trip PANE-NO-BLEED");
    assert_eq!(v.id, "PANE-NO-BLEED");
}

#[test]
fn fires_when_a_strip_kind_is_claimed_without_a_new_journal_step() {
    let mut prev = base_snapshot("abc  ");
    prev.focus = Pane::Explorer;
    prev.journal_pos = 5;
    prev.journal_len = 5;
    let mut next = base_snapshot("abc");
    next.focus = Pane::Explorer;
    next.version = 2;
    next.journal_pos = 5;
    next.journal_len = 5;
    next.newest_applied_edit_kind = Some(EditKind::StripTrailingWhitespace);
    let v = pane_no_bleed(&prev, &next, &key_ctx()).expect(
        "a mutation that journalled nothing is a bleed however the newest step is labelled",
    );
    assert_eq!(v.id, "PANE-NO-BLEED");
}
