//! Tests for `emit::mod`'s core machinery (`push_span_split_by_line`,
//! `emit` itself) — split out from `mod.rs` to keep it under the 500-line
//! budget. The claim primitive's own tests live with `EmitOut` in
//! `emit::claim`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::element::doc::DocMachine;
use rune_core::buffer::Buffer;
use rune_core::coords::BufferPoint;
use rune_core::cursor::CursorSet;

/// Proves `push_span_split_by_line`'s strict-invariants-gated assert
/// actually fires when a second visible claim overlaps a byte an
/// earlier one already claimed — the exact shape an empty list item's
/// marker running onto its continuation line produced ("- \n  > q")
/// before the root-cause fix (clamping the marker to its own line).
/// This crate's own test binary always has the gate armed (tied to
/// `cfg(test)`, not `cfg(debug_assertions)` — see the `emit` module
/// docs), so this fires in `cargo test --release` too.
fn test_out<'a>(
    spans: &'a mut [Vec<SyntaxSpan>],
    hidden: &'a mut Accounted,
    accounted: &'a mut Accounted,
    tables: &'a mut [Option<TableRowInfo>],
    decors: &'a mut [Option<LineDecor>],
    icons: &'a IconSet,
) -> EmitOut<'a> {
    EmitOut::new(
        Sinks {
            spans,
            hidden,
            accounted,
        },
        tables,
        80,
        icons,
        decors,
    )
}

#[test]
#[should_panic(expected = "already-claimed byte")]
fn push_span_split_by_line_asserts_on_duplicate_visible_claim() {
    let content = "abcdefgh\n";
    let starts = vec![0usize, 9];
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
    let mut hidden: Accounted = vec![Vec::new()];
    let mut accounted: Accounted = vec![Vec::new()];
    let mut tables: Vec<Option<TableRowInfo>> = vec![None];
    let mut decors: Vec<Option<LineDecor>> = vec![None];
    let icons = IconSet::unicode();
    let mut out = test_out(
        &mut spans,
        &mut hidden,
        &mut accounted,
        &mut tables,
        &mut decors,
        &icons,
    );

    push_span_split_by_line(
        content,
        &starts,
        ByteRange::new(2, 6),
        style::text_scope(),
        RevealState::Revealed,
        &mut out,
    );
    // Overlaps the [2,6) already claimed above.
    push_span_split_by_line(
        content,
        &starts,
        ByteRange::new(0, 8),
        style::text_scope(),
        RevealState::Revealed,
        &mut out,
    );
}

/// Proves the char-boundary-snap-and-surface path (verification round 3
/// MAJOR: `content.get(s..e)` returning `None` for a producer's
/// non-boundary range used to hit `else { continue }` and silently drop
/// the whole span — user bytes vanishing from the display) actually
/// fires under strict invariants, rather than being silently absorbed.
/// The exact shape a wikilink label range used to produce for a
/// multibyte final char ("[[ 日]]"): a range whose end lands mid-char.
#[test]
#[should_panic(expected = "not on a char boundary")]
fn push_span_split_by_line_asserts_on_non_char_boundary_span() {
    // "a日\n": 'a' is byte 0, '日' occupies bytes [1,4), '\n' is byte 4.
    let content = "a日\n";
    let starts = vec![0usize, content.len()];
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
    let mut hidden: Accounted = vec![Vec::new()];
    let mut accounted: Accounted = vec![Vec::new()];
    let mut tables: Vec<Option<TableRowInfo>> = vec![None];
    let mut decors: Vec<Option<LineDecor>> = vec![None];
    let icons = IconSet::unicode();
    let mut out = test_out(
        &mut spans,
        &mut hidden,
        &mut accounted,
        &mut tables,
        &mut decors,
        &icons,
    );

    push_span_split_by_line(
        content,
        &starts,
        ByteRange::new(0, 2), // byte 2 sits inside '日' — not a char boundary
        style::text_scope(),
        RevealState::Revealed,
        &mut out,
    );
}

/// The snap-outward arithmetic itself (used by the `else` branch above to
/// recover a valid, verbatim span instead of dropping one): for every
/// byte index in a multi-width string, `floor_char_boundary` rounds DOWN
/// to a valid boundary and `ceil_char_boundary` rounds UP to one — never
/// panicking, and never landing outside `[floor, idx] `/`[idx, ceil]`
/// respectively. This is what makes "snap outward and emit verbatim"
/// always safe: the recovered range only ever grows to include a few
/// more of the user's own bytes, never shrinks past what was asked.
#[test]
fn floor_and_ceil_char_boundary_snap_outward_never_split_a_char() {
    let content = "a日b👍c";
    for idx in 0..=content.len() {
        let f = content.floor_char_boundary(idx);
        let c = content.ceil_char_boundary(idx);
        assert!(
            content.is_char_boundary(f),
            "floor({idx}) = {f} not a boundary"
        );
        assert!(
            content.is_char_boundary(c),
            "ceil({idx}) = {c} not a boundary"
        );
        assert!(f <= idx, "floor({idx}) = {f} rounded UP, not down");
        assert!(c >= idx, "ceil({idx}) = {c} rounded DOWN, not up");
    }
}

#[test]
fn for_each_line_slice_skips_the_empty_trailing_slice_after_the_last_real_line() {
    // A malformed-but-defended-against producer range: `starts` models two
    // "lines" — `"abc\n"` and the zero-length line right after the final
    // newline — and the range reaches past both, to byte 6, on a 4-byte
    // buffer (the same "range extends past its own content" shape this
    // function's own docs cite for `ThematicBreak`). The last iterated line
    // (line 1) then has NO real overlap at all: `seg_start` and `seg_end`
    // both land on 4. `f` must not be called for it.
    let content = "abc\n";
    let starts = vec![0usize, content.len()];
    let mut calls: Vec<(usize, usize, usize)> = Vec::new();
    for_each_line_slice(content, &starts, ByteRange::new(2, 6), |ll| {
        calls.push((ll.line(), ll.start(), ll.end()));
    });
    assert_eq!(
        calls,
        vec![(0, 2, 3)],
        "the exhausted trailing line must not receive a zero-length callback"
    );
}

#[test]
#[should_panic(expected = "not on a char boundary")]
fn build_line_span_asserts_when_only_the_start_needs_snapping() {
    // "日b": '日' occupies bytes [0,3), 'b' is byte 3. `s = 1` is mid-char
    // (snaps down to 0, a real mismatch); `e = 3` already sits on a
    // boundary (no mismatch). `snapped_ok` must be the conjunction of BOTH
    // comparisons — false here — so this must still panic even though the
    // second half of the pair matches.
    let content = "日b";
    let _ = build_line_span(content, 0, 1, 3, style::text_scope(), RevealState::Rendered);
}

#[test]
fn build_line_span_treats_a_reversed_but_boundary_aligned_range_as_snap_ok() {
    // "abc" is pure ASCII, so every index 0..=3 is already a char boundary
    // on both sides of the reversed range below — `snapped_ok` is true and
    // no panic fires. `content.get(2..1)` still fails (start > end), so the
    // function falls through its `?` to `None` rather than panicking. Any
    // flip of either `==` in `snapped_ok` makes ONE side of the (already
    // boundary-aligned) pair report a spurious mismatch and panic instead.
    let content = "abc";
    let result = build_line_span(content, 0, 2, 1, style::text_scope(), RevealState::Rendered);
    assert!(result.is_none());
}

pub(crate) fn synced(content: &str, cursor_offset: usize, focused: bool) -> (Buffer, DocMachine) {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_reveal_mode(focused.into());
    doc.sync_content(&buf);
    let cursors = CursorSet::new(cursor_offset);
    doc.sync_cursors(&buf, &cursors, &[]);
    (buf, doc)
}

#[test]
fn rendered_span_cell_map_offsets_are_within_range() {
    let content = "**bold** text\n";
    let (buf, doc) = synced(content, content.len(), true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    for span in &lines[0].spans {
        if let SyntaxSpan::Substituted { cell_map, .. } = span {
            let range = span.range();
            for &off in cell_map {
                if let Some(off) = off {
                    let off = off as usize;
                    assert!(off < range.end);
                    assert!(off >= range.start);
                }
            }
        }
    }
}

#[test]
fn buffer_to_syntax_roundtrip_on_cursor_legal_position() {
    // Cursor is at end-of-buffer, well outside "**bold**"'s range, so
    // the emphasis is concealed on line 0 and its "**" delimiters (buffer
    // cols [0,2) and [6,8)) are NOT cursor-legal. Buffer col 8 is the
    // space right after the closing "**", a position with no hidden
    // delimiter on either side — genuinely cursor-legal, so the
    // roundtrip must be exact (unlike a position inside a hidden range,
    // which only guarantees the weaker stability invariant the other
    // test below checks).
    let content = "**bold** text\n";
    let (buf, doc) = synced(content, content.len(), true);
    let (_lines, snap) = emit(buf.content(), doc.blocks(), 80);
    let bp = BufferPoint { line: 0, col: 8 }; // buffer col 8 = the space after "**bold**"
    let sp = snap.buffer_to_syntax(bp);
    let bp2 = snap.syntax_to_buffer(sp);
    assert_eq!(bp, bp2);
}

#[test]
fn buffer_to_syntax_clamps_stably_inside_hidden_delimiter() {
    // Stability property: a position inside a hidden delimiter range does
    // NOT roundtrip to itself, but the CLAMPED position it lands on must
    // be idempotent.
    let content = "**bold** text\n";
    let (buf, doc) = synced(content, content.len(), true);
    let (_lines, snap) = emit(buf.content(), doc.blocks(), 80);
    let bp = BufferPoint { line: 0, col: 0 }; // inside the "**" open delimiter
    let sp = snap.buffer_to_syntax(bp);
    let bp2 = snap.syntax_to_buffer(sp);
    assert_ne!(
        bp, bp2,
        "col 0 sits inside a hidden delimiter, not cursor-legal"
    );
    let sp2 = snap.buffer_to_syntax(bp2);
    assert_eq!(
        sp, sp2,
        "the clamped position must be stable under a second round-trip"
    );
}

#[test]
fn image_reveal_state_selects_between_raw_markup_and_its_visible_label() {
    // `emit_inline`'s `Inline::Image` arm branches on `m.sm.state() ==
    // RevealState::Revealed`; a `==`-to-`!=` corruption swaps which side
    // runs for a GIVEN state, so driving the same content through both a
    // Revealed and a Rendered sync catches it either way the swap goes.
    let content = "![alt](http://x)\n";

    let (buf, doc) = synced(content, 0, true); // cursor inside the image's own range -> Revealed
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let revealed: String = lines[0]
        .spans
        .iter()
        .map(|s| s.text(buf.content()))
        .collect();
    assert_eq!(revealed, "![alt](http://x)");

    let (buf2, doc2) = synced(content, content.len(), false); // unfocused -> forced Rendered
    let (lines2, _snap2) = emit(buf2.content(), doc2.blocks(), 80);
    let rendered: String = lines2[0]
        .spans
        .iter()
        .map(|s| s.text(buf2.content()))
        .collect();
    assert_eq!(rendered, "alt");
}

/// WP2.S6 fixture, run BEFORE wiring list decor: settles whether comrak's
/// `ListItemM::marker` range for a NESTED item includes its leading indent
/// bytes. It does not — `"  - nested"`'s marker is `[8,10)` = `"- "`, two
/// bytes AFTER the two-space indent at `[6,8)`, which is why the indent is
/// never hidden and always rides through as an ordinary visible span. A
/// bullet-decor piece therefore needs no indent folded into it: the indent
/// stays in the line's own spans, unaffected by which glyph the decor
/// channel prefixes.
#[test]
fn nested_list_item_marker_excludes_leading_indent() {
    let content = "- top\n  - nested\n";
    let blocks = crate::parse::parse(content);

    fn find_list(blocks: &[crate::element::block::Block]) -> Option<&crate::element::block::ListM> {
        blocks.iter().find_map(|b| match b {
            crate::element::block::Block::List(l) => Some(l),
            _ => None,
        })
    }

    let top_list = find_list(&blocks).expect("top-level list");
    let top_item = &top_list.items[0];
    assert_eq!(&content[top_item.marker.start..top_item.marker.end], "- ");

    let nested_list = find_list(&top_item.children).expect("nested list");
    let nested_item = &nested_list.items[0];
    // The marker starts at the "-", not at the indent two bytes earlier.
    assert_eq!(
        &content[nested_item.marker.start..nested_item.marker.end],
        "- "
    );
    assert_eq!(content.as_bytes()[nested_item.marker.start], b'-');
}
