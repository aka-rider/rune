//! Fixed behavior for setext headings, at the span level.
//!
//! Per CommonMark, a lone `-` line right after paragraph text is a setext
//! heading underline, so comrak reports the paragraph as `Heading{level:2}`.
//! `Block::Heading`'s concealed arm now hides the underline row through the
//! same `hide_range` the thematic break uses (`HeadingM::underline`, the
//! setext heading's own underline row, when it is safe to conceal — see
//! that field's own docs), and paints it with a full-width rule in the
//! heading's own style.
//!
//! `HeadingM::sync` decides reveal from
//! `cursors.any_in_lines(self.line, self.last_line)` — the heading's own
//! line span, text and underline together — so a cursor parked on the
//! underline row reveals the whole heading too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use conceal_common::{joined_line, synced};
use rune_md::emit::emit;
use rune_syntax::element::RevealState;

/// (content, underline_row, description) — the four fixtures the task
/// requires: top-level, the user's real list-item shape, nested in a
/// blockquote, and an ATX control that must stay correct.
const TOP_LEVEL: &str = "Title\n---\nbody\n";
const LIST_ITEM_SHAPE: &str = "- **a**: b\n  -\n- next\n";
const BLOCKQUOTE: &str = "> a\n> ---\n";
const ATX_CONTROL: &str = "## Title\nbody\n";
/// Multi-line setext text — the underline is `content_lines`'s LAST entry,
/// never a fixed index: this fixture would trip an off-by-one that assumed
/// `content_lines[1]` is always the underline.
const MULTI_LINE_TEXT: &str = "Foo\nBar\n---\nbody\n";
/// `===` underlines a level-1 heading, `---` a level-2 one — both are
/// `setext`, so this pins the level-1 half of that split.
const LEVEL_ONE: &str = "Title\n===\nbody\n";

fn heading_reveal_state(doc: &rune_md::element::doc::DocMachine) -> RevealState {
    use rune_md::element::block::Block;

    fn find(blocks: &[Block]) -> Option<RevealState> {
        for b in blocks {
            match b {
                Block::Heading(_) => return Some(b.reveal_state()),
                Block::Blockquote(bq) => {
                    if let Some(s) = find(&bq.children) {
                        return Some(s);
                    }
                }
                Block::List(list) => {
                    for item in &list.items {
                        if let Some(s) = find(&item.children) {
                            return Some(s);
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }
    find(doc.blocks()).expect("fixture must contain a Heading block")
}

fn underline_line_offset(content: &str, underline_row: usize) -> usize {
    content
        .split('\n')
        .take(underline_row)
        .map(|l| l.len() + 1)
        .sum()
}

#[test]
fn setext_underline_row_is_hidden_while_concealed() {
    let (buf, doc) = synced(TOP_LEVEL, 0, false);
    let (_lines, snap) = emit(buf.content(), doc.blocks(), 80);
    let underline_len = buf.line(1).len();
    assert_eq!(
        snap.hidden_byte_count(1),
        underline_len,
        "a concealed setext heading hides its underline row's bytes entirely, matching ATX marker-hiding"
    );
}

#[test]
fn setext_underline_row_is_concealed_from_base_prose_scope() {
    let (buf, doc) = synced(TOP_LEVEL, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let joined = joined_line(&lines, 1, buf.content());
    assert_eq!(
        joined, "",
        "the underline row's raw bytes are hidden, not re-emitted verbatim by fill_gaps"
    );
}

#[test]
fn setext_underline_row_in_list_item_is_hidden_while_concealed() {
    let (buf, doc) = synced(LIST_ITEM_SHAPE, 0, false);
    let (_lines, snap) = emit(buf.content(), doc.blocks(), 80);
    let underline_len = buf.line(1).len();
    assert_eq!(
        snap.hidden_byte_count(1),
        underline_len,
        "the user's real shape (setext heading nested in a list item) also hides its underline row"
    );
}

#[test]
fn setext_underline_row_in_blockquote_hides_marker_and_underline_together() {
    // The blockquote's own per-line marker hiding is a SEPARATE, correctly
    // working mechanism (`BlockquoteMarkerM`) — it hides the "> " bytes,
    // and the heading's own `underline` hides the "---" that follows: the
    // whole row (2 + 3 = 5 bytes) is claimed hidden between the two.
    let (buf, doc) = synced(BLOCKQUOTE, 0, false);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(
        snap.hidden_byte_count(1),
        buf.line(1).len(),
        "the \"> \" marker and the \"---\" underline are both hidden"
    );
    assert_eq!(
        joined_line(&lines, 1, buf.content()),
        "",
        "no raw underline text leaks past the blockquote marker hiding"
    );
}

#[test]
fn cursor_on_setext_underline_reveals_the_heading() {
    let offset = underline_line_offset(TOP_LEVEL, 1);
    let (_buf, doc) = synced(TOP_LEVEL, offset, true);
    assert_eq!(
        heading_reveal_state(&doc),
        RevealState::Revealed,
        "HeadingM::sync keys off the heading's own line span, so a cursor on the underline row reveals it too"
    );
}

#[test]
fn cursor_on_setext_underline_in_list_item_reveals_the_heading() {
    let offset = underline_line_offset(LIST_ITEM_SHAPE, 1);
    let (_buf, doc) = synced(LIST_ITEM_SHAPE, offset, true);
    assert_eq!(
        heading_reveal_state(&doc),
        RevealState::Revealed,
        "the underline-row cursor reveal also works inside a list item"
    );
}

#[test]
fn setext_underline_row_is_hidden_with_multi_line_heading_text() {
    // The underline is line 2 here ("Foo"=0, "Bar"=1, "---"=2), not
    // `content_lines[1]` — pins that the underline is derived from
    // `content_lines.last()`, not a fixed index.
    let (buf, doc) = synced(MULTI_LINE_TEXT, 0, false);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(snap.hidden_byte_count(2), buf.line(2).len());
    assert_eq!(joined_line(&lines, 2, buf.content()), "");
}

#[test]
fn cursor_on_underline_reveals_multi_line_setext_heading() {
    let offset = underline_line_offset(MULTI_LINE_TEXT, 2);
    let (_buf, doc) = synced(MULTI_LINE_TEXT, offset, true);
    assert_eq!(heading_reveal_state(&doc), RevealState::Revealed);
}

#[test]
fn setext_level_one_underline_row_is_hidden_while_concealed() {
    let (buf, doc) = synced(LEVEL_ONE, 0, false);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(snap.hidden_byte_count(1), buf.line(1).len());
    assert_eq!(joined_line(&lines, 1, buf.content()), "");
}

#[test]
fn cursor_on_underline_reveals_level_one_setext_heading() {
    let offset = underline_line_offset(LEVEL_ONE, 1);
    let (_buf, doc) = synced(LEVEL_ONE, offset, true);
    assert_eq!(heading_reveal_state(&doc), RevealState::Revealed);
}

#[test]
fn cursor_on_heading_text_line_reveals_the_setext_heading() {
    // Control: a cursor on the heading's own FIRST line still reveals it —
    // `any_in_lines(first, last)` covers the text line just as
    // `any_on_line(first)` used to.
    let (_buf, doc) = synced(TOP_LEVEL, 0, true);
    assert_eq!(heading_reveal_state(&doc), RevealState::Revealed);
}

#[test]
fn atx_heading_marker_row_is_fully_hidden_while_concealed() {
    // Control: an ATX heading's `marker` range is non-empty (it covers
    // "## "), so its underline-less single row is hidden correctly — this
    // must stay green whatever the setext fix ends up doing.
    let (buf, doc) = synced(ATX_CONTROL, 0, false);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "Title");
    assert!(snap.hidden_byte_count(0) > 0);
}

#[test]
fn cursor_on_atx_heading_line_reveals_it() {
    // Control: ATX heading has no separate underline row, so the
    // first-line-only reveal check is correct there — must stay green.
    let (_buf, doc) = synced(ATX_CONTROL, 0, true);
    assert_eq!(heading_reveal_state(&doc), RevealState::Revealed);
}
