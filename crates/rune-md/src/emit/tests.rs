//! Tests for `emit::mod`'s core machinery (`unclaimed_subranges`,
//! `push_span_split_by_line`, `emit` itself) — split out from `mod.rs` to
//! keep it under CONSTITUTION §1.6's 500-LoC limit.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::element::doc::DocMachine;
use rune_core::buffer::Buffer;
use rune_core::coords::BufferPoint;
use rune_core::cursor::CursorSet;

/// The visible-side dedup computation, tested in isolation (no assert
/// involved — `unclaimed_subranges` itself never panics, it just
/// computes what's left). Mirrors "- \n  > q"'s shape: a claim
/// ([0,8)) that overlaps a bit already claimed in the middle ([2,6)),
/// leaving two disjoint unclaimed pieces.
#[test]
fn unclaimed_subranges_skips_already_claimed_bytes() {
    let pieces = unclaimed_subranges(0, 8, &[(2, 6)]);
    assert_eq!(pieces, vec![(0, 2), (6, 8)]);

    // Fully covered: nothing left.
    assert_eq!(
        unclaimed_subranges(2, 6, &[(0, 8)]),
        Vec::<(usize, usize)>::new()
    );

    // Disjoint existing claim: the whole requested range survives.
    assert_eq!(unclaimed_subranges(0, 4, &[(10, 12)]), vec![(0, 4)]);

    // Overlapping, unsorted, and touching existing entries are all
    // handled via the same `merge_overlapping` the hidden side uses.
    assert_eq!(
        unclaimed_subranges(0, 10, &[(6, 8), (1, 3), (3, 4)]),
        vec![(0, 1), (4, 6), (8, 10)]
    );
}

/// Proves `push_span_split_by_line`'s `STRICT_INVARIANTS`-gated assert
/// actually fires when a second visible claim overlaps a byte an
/// earlier one already claimed — the exact shape an empty list item's
/// marker running onto its continuation line produced ("- \n  > q")
/// before the root-cause fix (clamping the marker to its own line).
/// This crate's own test binary always has `STRICT_INVARIANTS = true`
/// (tied to `cfg(test)`, not `cfg(debug_assertions)` — see the
/// `emit` module docs), so this fires in `cargo test --release` too.
#[test]
#[should_panic(expected = "already-claimed byte")]
fn push_span_split_by_line_asserts_on_duplicate_visible_claim() {
    let content = "abcdefgh\n";
    let starts = vec![0usize, 9];
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
    let mut accounted: Accounted = vec![Vec::new()];

    push_span_split_by_line(
        content,
        &starts,
        ByteRange::new(2, 6),
        style::text_scope(),
        RevealState::Revealed,
        &mut spans,
        &mut accounted,
    );
    // Overlaps the [2,6) already claimed above.
    push_span_split_by_line(
        content,
        &starts,
        ByteRange::new(0, 8),
        style::text_scope(),
        RevealState::Revealed,
        &mut spans,
        &mut accounted,
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
    let mut accounted: Accounted = vec![Vec::new()];

    push_span_split_by_line(
        content,
        &starts,
        ByteRange::new(0, 2), // byte 2 sits inside '日' — not a char boundary
        style::text_scope(),
        RevealState::Revealed,
        &mut spans,
        &mut accounted,
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
        let f = floor_char_boundary(content, idx);
        let c = ceil_char_boundary(content, idx);
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

pub(crate) fn synced(content: &str, cursor_offset: usize, focused: bool) -> (Buffer, DocMachine) {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_reveal_mode(focused);
    doc.sync_content(&buf);
    let cursors = CursorSet::new(cursor_offset);
    doc.sync_cursors(&buf, &cursors);
    (buf, doc)
}

#[test]
fn heading_marker_hidden_when_not_on_cursor_line() {
    let (buf, doc) = synced("# hi\nsecond\n", 8, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let joined: String = lines[0]
        .spans
        .iter()
        .map(|s| s.text(buf.content()))
        .collect();
    assert_eq!(joined, "hi", "marker must be concealed off-cursor-line");
}

#[test]
fn heading_marker_revealed_on_cursor_line() {
    let (buf, doc) = synced("# hi\nsecond\n", 0, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let joined: String = lines[0]
        .spans
        .iter()
        .map(|s| s.text(buf.content()))
        .collect();
    assert_eq!(joined, "# hi");
}

#[test]
fn unfocused_conceals_everything() {
    let (buf, doc) = synced("# hi\n", 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let joined: String = lines[0]
        .spans
        .iter()
        .map(|s| s.text(buf.content()))
        .collect();
    assert_eq!(joined, "hi");
}

#[test]
fn code_fence_whole_block_reveals_as_unit() {
    let content = "```rust\nfn f() {}\n```\n";
    let (buf, doc) = synced(content, content.find("fn").unwrap(), true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    // Cursor is on line 1 (the content line); the whole 3-line fence
    // block must reveal, including the fence marker lines.
    assert_eq!(lines[0].spans[0].text(content), "```rust");
    assert_eq!(lines[1].spans[0].text(content), "fn f() {}");
    assert_eq!(lines[2].spans[0].text(content), "```");
}

#[test]
fn code_fence_conceals_marker_lines_off_cursor() {
    let content = "```rust\nfn f() {}\n```\nafter\n";
    let (buf, doc) = synced(content, content.find("after").unwrap(), true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    // Fence marker lines collapse to empty; content line shows verbatim.
    assert_eq!(
        lines[0]
            .spans
            .iter()
            .map(|s| s.text(content))
            .collect::<String>(),
        ""
    );
    assert_eq!(lines[1].spans[0].text(content), "fn f() {}");
}

#[test]
fn bold_reveals_with_nested_link_as_a_unit() {
    let content = "**[bo*ld*](url)** end\n";
    let cursor = content.find("ld").unwrap();
    let (buf, doc) = synced(content, cursor, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let joined: String = lines[0].spans.iter().map(|s| s.text(content)).collect();
    assert_eq!(joined, "**[bo*ld*](url)** end");
}

#[test]
fn bold_conceals_but_still_shows_nested_link_text() {
    let content = "**[bo*ld*](url)** end\n";
    let (buf, doc) = synced(content, content.len(), true); // cursor at " end", not inside bold
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let joined: String = lines[0].spans.iter().map(|s| s.text(content)).collect();
    assert_eq!(joined, "bold end");
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
                assert!(off == -1 || (off as usize) < range.end);
                if off != -1 {
                    assert!((off as usize) >= range.start);
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
    // Mirrors Go's FuzzSyntaxMapRoundtrip stability property: a
    // position inside a hidden delimiter range does NOT roundtrip to
    // itself, but the CLAMPED position it lands on must be idempotent.
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

/// `CellMap` is per-CHAR (one entry per `char`, `chars().count()` entries)
/// despite the name — easy to conflate with a terminal CELL (§1.5) in a
/// module where finding 1 (`table::layout`'s width bug) proves those two
/// units genuinely diverge. Pins the length against BOTH a combining-mark
/// cluster (two `char`s, one grapheme cluster) and a double-width CJK char
/// (one `char`, two display cells): the map has one entry per `char` in
/// every case, never one per grapheme cluster and never one per display
/// cell.
#[test]
fn build_cell_map_has_one_entry_per_char_not_per_grapheme_or_display_cell() {
    // "é" as `e` + COMBINING ACUTE ACCENT: 2 `char`s, ONE grapheme cluster.
    let combining = "e\u{0301}";
    let cm = build_cell_map(10, combining);
    assert_eq!(cm.len(), combining.chars().count());
    assert_eq!(cm, vec![10, 11]); // 'e' is 1 byte; the combining mark starts at 11

    // "世界": each char is ONE 3-byte codepoint but TWO display cells
    // (§1.5) — the map still has exactly one entry per char, at each
    // char's own byte start, never per display cell.
    let cjk = "世界";
    let cm = build_cell_map(0, cjk);
    assert_eq!(cm.len(), cjk.chars().count());
    assert_eq!(cm, vec![0, 3]);
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
