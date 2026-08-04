//! Characterizes a setext-heading rendering defect at the span level
//! (repro only — no production fix here).
//!
//! Per CommonMark, a lone `-` line right after paragraph text is a setext
//! heading underline, so comrak reports the paragraph as `Heading{level:2}`.
//! `Block::Heading`'s concealed arm hides only `h.marker`, which is an EMPTY
//! range for a setext heading (see `HeadingM`'s own doc comment on
//! `content_lines` in `crates/rune-md/src/element/block.rs`) — the
//! underline row's bytes are never claimed, so `fill_gaps` re-emits them
//! verbatim in the base prose scope: an unstyled `---`/`-` leftover sitting
//! under a concealed, icon-decorated heading.
//!
//! Second half: `HeadingM::sync` decides reveal from
//! `cursors.any_on_line(self.line)`, where `line` is the heading's FIRST
//! line — a cursor parked on the underline row never reveals the heading.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use conceal_common::{joined_line, synced};
use rune_md::emit::emit;
use rune_syntax::element::RevealState;
use rune_syntax::scope::scope_table;

/// (content, underline_row, description) — the four fixtures the task
/// requires: top-level, the user's real list-item shape, nested in a
/// blockquote, and an ATX control that must stay correct.
const TOP_LEVEL: &str = "Title\n---\nbody\n";
const LIST_ITEM_SHAPE: &str = "- **a**: b\n  -\n- next\n";
const BLOCKQUOTE: &str = "> a\n> ---\n";
const ATX_CONTROL: &str = "## Title\nbody\n";

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

fn dump_document(content: &str, underline_row: usize) {
    for &(focused, cursor_offset, label) in &[
        (false, 0, "concealed"),
        (
            true,
            underline_line_offset(content, underline_row),
            "cursor-on-underline",
        ),
    ] {
        let (buf, doc) = synced(content, cursor_offset, focused);
        let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
        let table = scope_table();
        eprintln!("--- document {content:?} [{label}] ---");
        for line in 0..buf.line_count() {
            let joined = joined_line(&lines, line, buf.content());
            let hidden = snap.hidden_byte_count(line);
            eprintln!("  row {line}: joined={joined:?} hidden_byte_count={hidden}");
            if let Some(l) = lines.get(line) {
                for (i, span) in l.spans.iter().enumerate() {
                    let scope_name = table.name(span.scope()).unwrap_or("?");
                    let reveal = if span.is_rendered() {
                        "Substituted(Rendered)"
                    } else {
                        "Identical(raw-bytes)"
                    };
                    eprintln!(
                        "    span {i}: scope={scope_name:?} range={:?} kind={reveal} text={:?}",
                        span.range(),
                        span.text(buf.content())
                    );
                }
            }
        }
        eprintln!("  heading reveal_state={:?}", heading_reveal_state(&doc));
    }
}

fn underline_line_offset(content: &str, underline_row: usize) -> usize {
    content
        .split('\n')
        .take(underline_row)
        .map(|l| l.len() + 1)
        .sum()
}

#[test]
fn dump_setext_rows() {
    dump_document(TOP_LEVEL, 1);
    dump_document(LIST_ITEM_SHAPE, 1);
    dump_document(BLOCKQUOTE, 1);
    dump_document(ATX_CONTROL, 0);
}

#[test]
fn setext_underline_row_is_not_hidden_while_concealed() {
    let (buf, doc) = synced(TOP_LEVEL, 0, false);
    let (_lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(
        snap.hidden_byte_count(1),
        0,
        "the underline row's bytes are never claimed as hidden by the concealed heading"
    );
}

#[test]
fn setext_underline_row_leaks_into_base_prose_scope_while_concealed() {
    let (buf, doc) = synced(TOP_LEVEL, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let joined = joined_line(&lines, 1, buf.content());
    assert_eq!(
        joined, "---",
        "the underline row is re-emitted verbatim by fill_gaps instead of being concealed with the heading"
    );
}

#[test]
fn setext_underline_row_in_list_item_is_not_hidden_while_concealed() {
    let (buf, doc) = synced(LIST_ITEM_SHAPE, 0, false);
    let (_lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(
        snap.hidden_byte_count(1),
        0,
        "the user's real shape (setext heading nested in a list item) also leaks its underline row"
    );
}

#[test]
fn setext_underline_row_in_blockquote_leaks_its_marker_text_unstyled() {
    // The blockquote's own per-line marker hiding is a SEPARATE, correctly
    // working mechanism (`BlockquoteMarkerM`) — it still hides the "> "
    // bytes (`hidden_byte_count(1) == 2`), so this can't assert zero
    // hidden bytes the way the top-level/list-item cases do. What's
    // defective is the "---" text itself: it survives as unstyled base
    // prose instead of being hidden or styled with the heading.
    let (buf, doc) = synced(BLOCKQUOTE, 0, false);
    let (lines, snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(
        snap.hidden_byte_count(1),
        2,
        "only the \"> \" marker is hidden"
    );
    assert_eq!(
        joined_line(&lines, 1, buf.content()),
        "---",
        "the underline row's own text still leaks unstyled past the blockquote marker hiding"
    );
}

#[test]
fn cursor_on_setext_underline_does_not_reveal_the_heading() {
    let offset = underline_line_offset(TOP_LEVEL, 1);
    let (_buf, doc) = synced(TOP_LEVEL, offset, true);
    assert_eq!(
        heading_reveal_state(&doc),
        RevealState::Rendered,
        "HeadingM::sync keys off the heading's first line only, so a cursor on the underline row leaves it concealed"
    );
}

#[test]
fn cursor_on_setext_underline_in_list_item_does_not_reveal_the_heading() {
    let offset = underline_line_offset(LIST_ITEM_SHAPE, 1);
    let (_buf, doc) = synced(LIST_ITEM_SHAPE, offset, true);
    assert_eq!(
        heading_reveal_state(&doc),
        RevealState::Rendered,
        "the underline-row cursor blind spot also reproduces inside a list item"
    );
}

#[test]
fn cursor_on_heading_text_line_reveals_the_setext_heading() {
    // Control: a cursor on the heading's own FIRST line does reveal it —
    // pins that `any_on_line(self.line)` works as intended for the line it
    // does check, isolating the defect to the underline row specifically.
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

#[test]
#[ignore = "documents the target behavior; unignore with the fix"]
fn setext_underline_row_is_hidden_while_concealed() {
    let (buf, doc) = synced(TOP_LEVEL, 0, false);
    let (_lines, snap) = emit(buf.content(), doc.blocks(), 80);
    let underline_len = buf.line(1).len();
    assert_eq!(
        snap.hidden_byte_count(1),
        underline_len,
        "a concealed setext heading should hide its underline row's bytes entirely, matching the ATX marker-hiding behavior"
    );
}

#[test]
#[ignore = "documents the target behavior; unignore with the fix"]
fn cursor_on_setext_underline_reveals_the_heading() {
    let offset = underline_line_offset(TOP_LEVEL, 1);
    let (_buf, doc) = synced(TOP_LEVEL, offset, true);
    assert_eq!(
        heading_reveal_state(&doc),
        RevealState::Revealed,
        "a cursor parked on the underline row should reveal the whole setext heading, not just its first line"
    );
}
