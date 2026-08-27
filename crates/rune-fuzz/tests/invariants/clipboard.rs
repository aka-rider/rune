//! WP6.S5 detection tests: `PASTE-VERBATIM`, `CLIP-OSC52`.

use rune_fuzz::invariant::{clip_osc52, paste_verbatim};
use rune_fuzz::step::MsgTag;
use rune_tui::focus::FocusTarget;
use rune_tui::keymap::{Command, KeyCode};
use rune_tui::pane::Pane;
use rune_tui::runtime::PasteTarget;

use crate::support::{
    base_ctx, base_snapshot, collapsed_cursor, key, other_doc_id, selection_cursor, sup,
};

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
    ctx.msg = MsgTag::ClipboardRead {
        text: "line1\r\nline2".to_string(),
        target: PasteTarget::Document(prev.active),
    };
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

/// WP5.S3's own regression: a title-targeted `ClipboardRead` never touches
/// the active document at all (the title has its own in-memory field, out
/// of this checker's domain), so a mismatch between `prev.content`
/// and `next.content` here must NOT trip `PASTE-VERBATIM` — without the
/// `target == PasteTarget::Document(prev.active)` guard it would, since a
/// title-targeted paste legitimately leaves the document untouched while
/// the checker's own `expected` computation still assumes it landed there.
#[test]
fn paste_verbatim_ignores_a_clipboard_read_targeted_at_the_title() {
    let mut prev = base_snapshot("ac");
    prev.focus = Pane::Title;
    let next = base_snapshot("ac"); // correctly untouched
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::ClipboardRead {
        text: "b".to_string(),
        target: PasteTarget::Title(prev.active),
    };
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

/// A `ClipboardRead` captured for some OTHER document (a switch mid-flight,
/// decision 11) must not be checked against `prev`, which describes the
/// document that is active NOW, not the one the reply was captured for.
#[test]
fn paste_verbatim_ignores_a_clipboard_read_targeted_at_a_different_document() {
    let prev = base_snapshot("ac");
    let next = base_snapshot("ac");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::ClipboardRead {
        text: "b".to_string(),
        target: PasteTarget::Document(other_doc_id()),
    };
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

/// A `MsgTag::Paste` step with focus on the title is checked against the
/// title field, never the document — the document must stay untouched
/// while the sanitized text lands in the title.
#[test]
fn paste_verbatim_checks_a_paste_landing_in_the_title_field() {
    let mut prev = base_snapshot("ac");
    prev.focus = Pane::Title;
    prev.focus_target = FocusTarget::Title;
    let mut next = base_snapshot("ac"); // document stays untouched
    next.focus = Pane::Title;
    next.focus_target = FocusTarget::Title;
    next.title_text = "b".to_string();
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("b".to_string());
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

#[test]
fn paste_verbatim_detects_a_title_paste_that_never_lands() {
    let mut prev = base_snapshot("ac");
    prev.focus = Pane::Title;
    prev.focus_target = FocusTarget::Title;
    let mut next = base_snapshot("ac");
    next.focus = Pane::Title;
    next.focus_target = FocusTarget::Title; // title_text left at "" -- wrong
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("b".to_string());
    let v = paste_verbatim(&prev, &next, &ctx)
        .expect("a title paste that never updates the title field must trip PASTE-VERBATIM");
    assert_eq!(v.id, "PASTE-VERBATIM");
}

#[test]
fn paste_verbatim_accepts_a_paste_over_a_selection() {
    // CODE-REVIEW.md rune-fuzz finding 12: a paste over a selection is the
    // byte-displacing path, previously skipped entirely.
    let mut prev = base_snapshot("hello world");
    prev.cursors = vec![selection_cursor(1, 0, 5)]; // "hello" selected
    let next = base_snapshot("bye world");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("bye".to_string());
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

#[test]
fn paste_verbatim_accepts_a_paste_over_a_reversed_selection() {
    let mut prev = base_snapshot("hello world");
    prev.cursors = vec![selection_cursor(1, 5, 0)];
    let next = base_snapshot("bye world");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("bye".to_string());
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

#[test]
fn paste_verbatim_detects_a_mismatched_selection_replace() {
    let mut prev = base_snapshot("hello world");
    prev.cursors = vec![selection_cursor(1, 0, 5)]; // "hello" selected
    let next = base_snapshot("hello world"); // wrong: selection never replaced
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("bye".to_string());
    let v = paste_verbatim(&prev, &next, &ctx)
        .expect("a paste that fails to replace the selection must trip PASTE-VERBATIM");
    assert_eq!(v.id, "PASTE-VERBATIM");
}

#[test]
fn paste_verbatim_checks_a_paste_landing_in_the_search_field() {
    let mut prev = base_snapshot("ac");
    prev.focus_target = FocusTarget::SearchField;
    prev.search_draft = Some("q".to_string());
    let mut next = base_snapshot("ac");
    next.focus_target = FocusTarget::SearchField;
    next.search_draft = Some("qb".to_string());
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("b".to_string());
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

#[test]
fn paste_verbatim_detects_a_swallowed_search_field_paste() {
    let mut prev = base_snapshot("ac");
    prev.focus_target = FocusTarget::SearchField;
    prev.search_draft = Some("q".to_string());
    let mut next = base_snapshot("ac");
    next.focus_target = FocusTarget::SearchField;
    next.search_draft = Some("q".to_string()); // wrong: never appended
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("b".to_string());
    let v = paste_verbatim(&prev, &next, &ctx)
        .expect("a search-field paste that never appends must trip PASTE-VERBATIM");
    assert_eq!(v.id, "PASTE-VERBATIM");
}

/// The file-search paste handler takes only the first line and strips
/// control characters before appending — a multi-line paste that sanitizes
/// down to an empty first line is a legitimate no-op, not a swallow.
#[test]
fn paste_verbatim_accepts_a_filesearch_paste_that_sanitizes_to_nothing() {
    let mut prev = base_snapshot("ac");
    prev.focus_target = FocusTarget::FileSearch;
    prev.filesearch_query = Some(String::new());
    let mut next = base_snapshot("ac");
    next.focus_target = FocusTarget::FileSearch;
    next.filesearch_query = Some(String::new());
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("\n\n\n".to_string());
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

#[test]
fn paste_verbatim_checks_a_paste_landing_in_the_filesearch_query() {
    let mut prev = base_snapshot("ac");
    prev.focus_target = FocusTarget::FileSearch;
    prev.filesearch_query = Some("re".to_string());
    let mut next = base_snapshot("ac");
    next.focus_target = FocusTarget::FileSearch;
    next.filesearch_query = Some("readme".to_string());
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("adme\nignored second line".to_string());
    assert_eq!(paste_verbatim(&prev, &next, &ctx), None);
}

#[test]
fn paste_verbatim_detects_a_swallowed_filesearch_paste() {
    let mut prev = base_snapshot("ac");
    prev.focus_target = FocusTarget::FileSearch;
    prev.filesearch_query = Some("re".to_string());
    let mut next = base_snapshot("ac");
    next.focus_target = FocusTarget::FileSearch;
    next.filesearch_query = Some("re".to_string()); // wrong: never appended
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("adme".to_string());
    let v = paste_verbatim(&prev, &next, &ctx)
        .expect("a file-search paste that never appends must trip PASTE-VERBATIM");
    assert_eq!(v.id, "PASTE-VERBATIM");
}

/// `route_bracketed_paste` refuses a bracketed paste while a chrome pane
/// (Explorer/Tabs/Messages) holds focus — the invariant flags a document
/// that changed anyway and accepts one left untouched.
#[test]
fn paste_verbatim_flags_a_paste_landing_in_the_document_from_the_explorer_pane() {
    let mut prev = base_snapshot("ac");
    prev.focus = Pane::Explorer;
    prev.focus_target = FocusTarget::Explorer;
    let mut next = base_snapshot("bac");
    next.focus = Pane::Explorer;
    next.focus_target = FocusTarget::Explorer;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("b".to_string());
    let violation =
        paste_verbatim(&prev, &next, &ctx).expect("a refused paste that lands must violate");
    assert_eq!(violation.id, "PASTE-VERBATIM");

    let mut untouched = base_snapshot("ac");
    untouched.focus = Pane::Explorer;
    untouched.focus_target = FocusTarget::Explorer;
    assert_eq!(paste_verbatim(&prev, &untouched, &ctx), None);
}

#[test]
fn paste_verbatim_ignores_a_paste_on_a_read_only_document() {
    // The Help virtual document (reachable since F1 joined arb_any_keycode,
    // CODE-REVIEW.md rune-fuzz finding 9) refuses every mutating command,
    // including paste, by construction.
    let mut prev = base_snapshot("ac");
    prev.read_only = rune_tui::document::ReadOnly::Always;
    let next = base_snapshot("ac"); // correctly untouched
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Paste("b".to_string());
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

#[test]
fn clip_osc52_ignores_a_key_swallowed_by_an_open_overlay() {
    let mut prev = base_snapshot("hello world");
    prev.cursors = vec![selection_cursor(1, 0, 5)]; // "hello"
    prev.focus_target = FocusTarget::Palette;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('c'), sup()),
        command: Some(Command::Copy),
    };
    ctx.raw = Vec::new();
    assert_eq!(clip_osc52(&prev, &ctx), None);
}
