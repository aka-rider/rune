//! Split off `conceal_roundtrip.rs` (WP11): the base reveal-parity
//! table tests and the per-byte coverage regression cases (review BLOCKER
//! 1/2/3, MAJOR 4) — every byte of every line is either visible or hidden,
//! never dropped.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use conceal_common::{assert_full_line_coverage, joined_line, synced};
use rune_core::coords::BufferPoint;
use rune_md::emit::emit;
use rune_syntax::element::RevealState;

// ---------------------------------------------------------------------
// (a) Reveal-parity table tests.
// ---------------------------------------------------------------------

#[test]
fn cursor_on_heading_line_reveals_marker() {
    let (buf, doc) = synced("## heading\nbody\n", 0, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "## heading");
}

#[test]
fn cursor_off_heading_line_conceals_marker() {
    let (buf, doc) = synced("## heading\nbody\n", "## heading\n".len(), true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "heading");
}

#[test]
fn cursor_inside_bold_reveals_with_nested_link_as_a_unit() {
    let content = "**[bo*ld*](url)** end\n";
    let cursor = content.find("ld").expect("fixture contains 'ld'");
    let (buf, doc) = synced(content, cursor, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(
        joined_line(&lines, 0, buf.content()),
        "**[bo*ld*](url)** end"
    );
}

#[test]
fn cursor_outside_bold_conceals_delimiters_but_keeps_nested_text() {
    let content = "**[bo*ld*](url)** end\n";
    let (buf, doc) = synced(content, content.len(), true); // cursor on " end"
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "bold end");
}

#[test]
fn cursor_inside_fence_reveals_whole_block_as_a_unit() {
    let content = "before\n```rust\nfn f() {}\n```\nafter\n";
    let cursor = content.find("fn f").expect("fixture contains code");
    let (buf, doc) = synced(content, cursor, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 1, buf.content()), "```rust");
    assert_eq!(joined_line(&lines, 2, buf.content()), "fn f() {}");
    assert_eq!(joined_line(&lines, 3, buf.content()), "```");
}

#[test]
fn cursor_outside_fence_conceals_fence_markers() {
    let content = "before\n```rust\nfn f() {}\n```\nafter\n";
    let (buf, doc) = synced(content, 0, true); // cursor on "before"
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 1, buf.content()), "");
    assert_eq!(joined_line(&lines, 2, buf.content()), "fn f() {}");
    assert_eq!(joined_line(&lines, 3, buf.content()), "");
}

#[test]
fn unfocused_renders_everything_concealed_even_on_cursor_line() {
    let content = "## heading\n**bold** text\n";
    // Cursor sits ON the heading line and inside the bold span — if focused,
    // both would reveal. Unfocused must force ForceRendered regardless
    // (Gotchas: "Unfocused -> ForceRendered").
    let (buf, doc) = synced(content, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "heading");
    assert_eq!(joined_line(&lines, 1, buf.content()), "bold text");
    for block in doc.blocks() {
        assert_eq!(block.reveal_state(), RevealState::Rendered);
    }
}

#[test]
fn tasklist_marker_reveals_on_cursor_line() {
    let content = "- [x] task\nother\n";
    let (buf, doc) = synced(content, 0, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "- [x] task");
}

#[test]
fn tasklist_marker_conceals_off_cursor_line() {
    // The "- " prefix conceals like any list marker, but the checkbox
    // itself substitutes to its glyph (`☑` for `[x]`) rather than
    // disappearing outright — the behavior this test pins.
    let content = "- [x] task\nother\n";
    let (buf, doc) = synced(content, "- [x] task\n".len(), true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "\u{2611} task");
}

#[test]
fn blockquote_marker_reveals_per_line_independently() {
    let content = "> line one\n> line two\n";
    let (buf, doc) = synced(content, 0, true); // cursor on line 0 only
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "> line one");
    assert_eq!(joined_line(&lines, 1, buf.content()), "line two");
}

// ---------------------------------------------------------------------
// (a2) Per-byte coverage regression cases (review BLOCKER 1/2/3, MAJOR 4):
// every byte of every line is either visible or hidden — never dropped.
// ---------------------------------------------------------------------

#[test]
fn trailing_whitespace_is_visible_not_dropped() {
    let (buf, doc) = synced("hello   \nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0, buf.content()), "hello   ");
}

#[test]
fn leading_indent_is_visible_not_dropped() {
    let (buf, doc) = synced("  leading spaces\nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0, buf.content()), "  leading spaces");
}

#[test]
fn embedded_tab_is_visible_not_dropped() {
    let (buf, doc) = synced("a\tb\nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0, buf.content()), "a\tb");
}

#[test]
fn whitespace_only_line_is_visible_not_dropped() {
    let (buf, doc) = synced("para\n   \nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
}

#[test]
fn indented_code_block_is_visible_not_dropped() {
    let (buf, doc) = synced("para\n\n    indented code\n\nafter\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 2, buf.content()), "    indented code");
}

#[test]
fn indented_list_marker_is_visible_not_dropped() {
    let (buf, doc) = synced("  - nested item\nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
}

#[test]
fn crlf_carriage_return_is_visible_not_dropped() {
    let (buf, doc) = synced("line one\r\nline two\r\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    // The bare \r before \n is user content — it must show up in
    // the concealed/revealed text exactly as written, never dropped.
    assert!(
        joined_line(&lines, 0, buf.content()).ends_with('\r'),
        "line 0 = {:?}",
        joined_line(&lines, 0, buf.content())
    );
}

#[test]
fn atx_heading_closing_sequence_is_visible_not_dropped() {
    // CommonMark strips an optional trailing "#"-run from an ATX heading's
    // CONTENT, but those trailing bytes are still part of the raw line —
    // they must show up as visible text when concealed, not vanish.
    let (buf, doc) = synced("## heading ##\nnext\n", "## heading ##\n".len(), true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
}

#[test]
fn backslash_escape_is_visible_not_dropped() {
    let (buf, doc) = synced("\\*not bold\\*\nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
}

#[test]
fn empty_link_hides_exactly_once() {
    // BLOCKER 2: an empty-text link's open/close delimiter fallbacks used to
    // both default to the whole token range, double-hiding it and breaking
    // buffer_to_syntax monotonicity.
    let content = "see [](http://x) here\n";
    let (buf, doc) = synced(content, 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0, buf.content()), "see  here");

    // buffer_to_syntax must be monotonic non-decreasing across the line.
    let mut prev = None;
    for col in 0..=content.trim_end_matches('\n').len() {
        let sp = snap.buffer_to_syntax(BufferPoint { line: 0, col });
        if let Some(p) = prev {
            assert!(
                sp.col >= p,
                "buffer_to_syntax not monotonic at col {col}: prev={p} now={}",
                sp.col
            );
        }
        prev = Some(sp.col);
    }
}

#[test]
fn unterminated_fence_keeps_every_line_visible_content() {
    // BLOCKER 3: `last_line > first_line` alone was wrongly treated as
    // "closing fence exists" — an in-progress (unterminated) fence lost its
    // last content line to a phantom fence_close.
    let content = "```rust\nfn f() {}\nlet x = 1;\n";
    let (buf, doc) = synced(content, 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    // Cursor away (unfocused-equivalent conceal): every content line must
    // still show its text — nothing after the opening fence is a phantom
    // closing marker.
    assert_eq!(joined_line(&lines, 1, buf.content()), "fn f() {}");
    assert_eq!(joined_line(&lines, 2, buf.content()), "let x = 1;");
}

#[test]
fn nested_blockquote_markers_are_at_their_true_depth_offset() {
    // MAJOR 4: both depths used to report marker range [0,2), double-hiding
    // the same 2 bytes and leaving the inner "> " at [2,4) unmodeled.
    let content = "> > nested quote\n";
    let (buf, doc) = synced(content, content.len(), true); // cursor away: both conceal
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0, buf.content()), "nested quote");
}
