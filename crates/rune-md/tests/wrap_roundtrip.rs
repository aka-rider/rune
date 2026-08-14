//! WP4.S2: round-trip proptest, plus pinned CJK/emoji/tab cases, for
//! `visual_col`/`byte_col_from_visual`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use proptest::prelude::*;
use rune_core::buffer::Buffer;
use rune_core::coords::SyntaxPoint;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;
use rune_md::emit::emit;
use rune_syntax::wrap::WrapMap;
use unicode_segmentation::UnicodeSegmentation;

fn arb_markdown_fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("plain text".to_string()),
        Just("**bold**".to_string()),
        Just("*italic*".to_string()),
        Just("~~strike~~".to_string()),
        Just("`code`".to_string()),
        Just("[link](url)".to_string()),
        Just("[[wiki|label]]".to_string()),
        Just("# heading".to_string()),
        Just("> quoted line".to_string()),
        Just("- item".to_string()),
        Just("- [x] done task".to_string()),
        Just("```\nfenced\ncontent\n```".to_string()),
        Just("word1 word2 word3 word4 word5".to_string()),
        "[a-zA-Z0-9 ]{0,10}".prop_map(|s| s),
    ]
}

fn arb_content() -> impl Strategy<Value = String> {
    proptest::collection::vec(arb_markdown_fragment(), 0..8).prop_map(|frags| frags.join("\n"))
}

/// Every grapheme-cluster boundary in a syntax line's own concatenated
/// visible text, plus the line's own end — the domain `wrap_to_syntax`
/// ([rune-syntax 3]) is now allowed to promise round-trip identity over.
/// A byte offset landing MID-cluster (e.g. inside the 3-byte checkbox
/// glyph `push_task_checkbox` substitutes) is not a position any real
/// cursor movement can ever produce, and `wrap_to_syntax` now snaps such
/// an offset down to the nearest cluster start rather than passing it
/// through unchanged — the fix this test's old `0..=syntax_len` domain
/// (every byte offset, boundary or not) was accidentally asserting AGAINST.
fn syntax_line_grapheme_bounds(
    lines: &[rune_syntax::SyntaxLine],
    line: usize,
    content: &str,
) -> Vec<usize> {
    let text: String = lines
        .get(line)
        .map(|l| l.spans.iter().map(|s| s.text(content)).collect())
        .unwrap_or_default();
    let mut bounds: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
    bounds.push(text.len());
    bounds
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn wrap_to_syntax_of_syntax_to_wrap_is_identity(
        content in arb_content(),
        raw_offset in any::<usize>(),
        focused in any::<bool>(),
        raw_width in 1u16..40,
    ) {
        let buf = Buffer::new(&content);
        let offset = if buf.is_empty() { 0 } else { raw_offset % (buf.len() + 1) };
        let mut doc = DocMachine::new();
        doc.set_reveal_mode(focused.into());
        doc.sync_content(&buf);
        let cursors = CursorSet::new(offset);
        doc.sync_cursors(&buf, &cursors);
        let (lines, _syntax_snap) = emit(buf.content(), doc.blocks(), 80);
        let wrap_snap = WrapMap::new(raw_width).sync(buf.content(), &lines);

        for line in 0..buf.line_count() {
            for col in syntax_line_grapheme_bounds(&lines, line, buf.content()) {
                let sp = SyntaxPoint { line, col };
                let wp = wrap_snap.syntax_to_wrap(sp);
                let sp2 = wrap_snap.wrap_to_syntax(buf.content(), wp);
                prop_assert_eq!(sp, sp2, "SyntaxToWrap/WrapToSyntax roundtrip failed: sp={:?} wp={:?} sp2={:?} width={}", sp, wp, sp2, raw_width);
            }
        }

        // Segment texts concatenate back to the exact syntax-space line
        // text, and no segment's VISUAL width exceeds the configured width
        // except a single over-wide char (Major 5: Rendered spans are NOT
        // exempt — a Rendered span's text is sliced at a break just like
        // any other span; only its buffer range stays whole. No whitelist
        // here anymore.).
        for line in 0..buf.line_count() {
            let mut joined = String::new();
            let mut row = wrap_snap.model_line_to_first_row(line);
            loop {
                if wrap_snap.row_to_model_line(row) != line {
                    break;
                }
                let seg_visual_width =
                    wrap_snap.visual_col(buf.content(), row, wrap_snap.segment_len_at(row));
                if seg_visual_width > raw_width as usize {
                    let segs = wrap_snap.segments();
                    if let Some(seg) = segs.get(row) {
                        let is_single_overwide_char = seg.spans.iter().map(|s| s.text(buf.content()).chars().count()).sum::<usize>() == 1;
                        prop_assert!(is_single_overwide_char, "segment at row {} exceeds width {} without being a single over-wide char: {:?}", row, raw_width, seg.spans.iter().map(|s| s.text(buf.content())).collect::<String>());
                    }
                }
                for sp in &wrap_snap.segments()[row].spans {
                    joined.push_str(sp.text(buf.content()));
                }
                row += 1;
                if row >= wrap_snap.total_rows() {
                    break;
                }
            }
            let expected = lines.get(line).map(|l| l.spans.iter().map(|s| s.text(buf.content())).collect::<String>()).unwrap_or_default();
            prop_assert_eq!(joined, expected);
        }
    }
}

// ---------------------------------------------------------------------
// Pinned CJK / emoji / tab cases for visual_col / byte_col_from_visual.
// ---------------------------------------------------------------------

fn wrap_for(
    content: &str,
    width: u16,
) -> (rune_core::buffer::Buffer, rune_syntax::wrap::WrapSnapshot) {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_reveal_mode(true.into());
    doc.sync_content(&buf);
    let cursors = CursorSet::new(0);
    doc.sync_cursors(&buf, &cursors);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let wrap = WrapMap::new(width).sync(buf.content(), &lines);
    (buf, wrap)
}

#[test]
fn cjk_double_width_visual_col_is_inverse_of_byte_col_from_visual() {
    let (buf, wrap) = wrap_for("汉字テスト\n", 80);
    // Each of these 5 chars is double-width (visual width 2), 3 bytes each.
    for byte_col in [0, 3, 6, 9, 12, 15] {
        let visual = wrap.visual_col(buf.content(), 0, byte_col);
        let back = wrap.byte_col_from_visual(buf.content(), 0, visual);
        assert_eq!(back, byte_col, "byte_col={byte_col} visual={visual}");
    }
    assert_eq!(wrap.visual_col(buf.content(), 0, 15), 10); // 5 double-width chars = 10 cols
}

#[test]
fn emoji_zwj_family_width_round_trips() {
    let content = "a \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466} b\n";
    let (buf, wrap) = wrap_for(content, 80);
    let line_len = wrap.segment_len_at(0);
    // visual_col is monotonic non-decreasing with byte_col, and
    // byte_col_from_visual never returns past the line length.
    let mut last_visual = 0usize;
    for byte_col in 0..=line_len {
        let visual = wrap.visual_col(buf.content(), 0, byte_col);
        assert!(visual >= last_visual);
        last_visual = visual;
        let back = wrap.byte_col_from_visual(buf.content(), 0, visual);
        assert!(back <= line_len);
    }
}

#[test]
fn tab_expands_to_next_stop_of_four() {
    let (buf, wrap) = wrap_for("a\tb\n", 80);
    // 'a' at visual col 0 (width 1); '\t' at visual col 1 expands to the
    // next stop-of-4, i.e. to visual col 4; 'b' follows at visual col 4.
    assert_eq!(wrap.visual_col(buf.content(), 0, 0), 0); // before 'a'
    assert_eq!(wrap.visual_col(buf.content(), 0, 1), 1); // before '\t', after 'a'
    assert_eq!(wrap.visual_col(buf.content(), 0, 2), 4); // before 'b', after the tab stop
    assert_eq!(wrap.byte_col_from_visual(buf.content(), 0, 4), 2); // 'b' starts at byte 2
}
