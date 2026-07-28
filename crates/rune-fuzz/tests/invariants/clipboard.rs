//! WP6.S5 detection tests: `PASTE-VERBATIM`, `CLIP-OSC52`.

use rune_fuzz::invariant::{clip_osc52, paste_verbatim};
use rune_fuzz::step::MsgTag;
use rune_tui::keymap::{Command, KeyCode};

use crate::support::{base_ctx, base_snapshot, collapsed_cursor, key, selection_cursor, sup};

// ---------------------------------------------------------------------
// PASTE-VERBATIM
// ---------------------------------------------------------------------

#[test]
fn paste_verbatim_detects_a_mismatched_insertion() {
    let prev = base_snapshot("ac"); // cursor at 0
    let next = base_snapshot("XXac"); // wrong: should be "bac"
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("b".to_string());
    let v = paste_verbatim(&prev, &next, &ctx)
        .expect("a paste not inserted verbatim at the caret must trip PASTE-VERBATIM");
    assert_eq!(v.id, "PASTE-VERBATIM");
}

#[test]
fn paste_verbatim_accepts_a_correct_verbatim_insertion() {
    let prev = base_snapshot("ac"); // cursor at 0
    let next = base_snapshot("bac");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("b".to_string());
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

#[test]
fn paste_verbatim_accepts_a_crlf_clipboard_read_verbatim() {
    let prev = base_snapshot("ac");
    let next = base_snapshot("line1\r\nline2ac");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::ClipboardRead("line1\r\nline2".to_string());
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

// ---------------------------------------------------------------------
// CLIP-OSC52
// ---------------------------------------------------------------------

#[test]
fn clip_osc52_detects_a_missing_raw_chunk() {
    let mut prev = base_snapshot("hello world");
    prev.cursors = vec![selection_cursor(1, 0, 5)]; // "hello"
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('c'), sup()),
        command: Some(Command::Copy),
    };
    ctx.raw = Vec::new(); // no OSC 52 chunk emitted at all
    let v = clip_osc52(&prev, &ctx)
        .expect("a Copy over a selection with no matching OSC 52 chunk must trip CLIP-OSC52");
    assert_eq!(v.id, "CLIP-OSC52");
}

#[test]
fn clip_osc52_detects_a_wrong_payload() {
    let mut prev = base_snapshot("hello world");
    prev.cursors = vec![selection_cursor(1, 0, 5)]; // "hello"
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('x'), sup()),
        command: Some(Command::Cut),
    };
    ctx.raw = vec![rune_tui::clipboard::osc52_copy(b"WRONG PAYLOAD")];
    let v = clip_osc52(&prev, &ctx)
        .expect("an OSC 52 chunk carrying the wrong payload must trip CLIP-OSC52");
    assert_eq!(v.id, "CLIP-OSC52");
}

#[test]
fn clip_osc52_accepts_the_correct_payload() {
    let mut prev = base_snapshot("hello world");
    prev.cursors = vec![selection_cursor(1, 0, 5)]; // "hello"
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('c'), sup()),
        command: Some(Command::Copy),
    };
    ctx.raw = vec![rune_tui::clipboard::osc52_copy(b"hello")];
    assert_eq!(clip_osc52(&prev, &ctx), None);
}

#[test]
fn clip_osc52_ignores_a_collapsed_cursor() {
    let mut prev = base_snapshot("hello world");
    prev.cursors = vec![collapsed_cursor(1, 3)]; // no selection
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('c'), sup()),
        command: Some(Command::Copy),
    };
    ctx.raw = Vec::new();
    assert_eq!(clip_osc52(&prev, &ctx), None);
}
