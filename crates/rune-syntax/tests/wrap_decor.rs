//! WP3.S5: `LineDecor` -> `SegDecor` integration over the wrap pass.
//! Hand-builds its own `SyntaxLine`s (mirroring `wrap/mod.rs`'s own inline
//! tests and `wrap_query_edges.rs`'s `one_span_line`) rather than routing
//! through `rune-md`'s emitter — the wrap pass is producer-agnostic (module
//! docs), so these five checks exercise `WrapMap`'s decor contract on its
//! own, no markdown involved.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_core::coords::SyntaxPoint;
use rune_syntax::decor::{DecorPiece, LineDecor};
use rune_syntax::scope::ScopeId;
use rune_syntax::syntax::{SyntaxLine, SyntaxSpan};
use rune_syntax::wrap::{WrapMap, grapheme_width};
use unicode_segmentation::UnicodeSegmentation;

const TEXT: ScopeId = ScopeId(0);
const DECOR: ScopeId = ScopeId(1);

fn one_span_line(content: &str, decor: Option<LineDecor>) -> SyntaxLine {
    SyntaxLine {
        spans: vec![SyntaxSpan::identical(content, TEXT, 0..content.len())],
        table: None,
        decor,
    }
}

fn empty_line(decor: Option<LineDecor>) -> SyntaxLine {
    SyntaxLine {
        spans: Vec::new(),
        table: None,
        decor,
    }
}

/// A list-bullet-shaped decor: a fixed-width piece on the first row, blank
/// padding of the same width on every continuation row — the common case
/// (heading icon, bullet, ordered number).
fn bullet_decor() -> LineDecor {
    LineDecor {
        pieces: vec![DecorPiece {
            first: "\u{2022} ".to_string(), // bullet + space, 2 cells
            cont: "  ".to_string(),
            scope: DECOR,
        }],
        is_rule: false,
    }
}

/// A blockquote-bar-shaped decor: the SAME glyph on every row, first and
/// continuation alike.
fn quote_bar_decor() -> LineDecor {
    LineDecor {
        pieces: vec![DecorPiece {
            first: "\u{258E}".to_string(), // 1 cell
            cont: "\u{258E}".to_string(),
            scope: DECOR,
        }],
        is_rule: false,
    }
}

fn rule_decor(cells: usize) -> LineDecor {
    LineDecor {
        pieces: vec![DecorPiece {
            first: "\u{2500}".repeat(cells), // full-width hr rule
            cont: String::new(),
            scope: DECOR,
        }],
        is_rule: true,
    }
}

fn row_content_cells(content: &str, seg: &rune_syntax::wrap::WrapSegment) -> usize {
    seg.spans
        .iter()
        .map(|s| s.text(content))
        .flat_map(|t| t.graphemes(true).map(grapheme_width).collect::<Vec<_>>())
        .sum()
}

// ---------------------------------------------------------------------
// (i) every segment of a decorated long line fits width INCLUDING decor.
// ---------------------------------------------------------------------

#[test]
fn every_segment_of_a_decorated_long_line_fits_width_including_decor_cells() {
    let content = "one two three four five six seven eight nine ten eleven twelve";
    let width = 12u16;
    let line = one_span_line(content, Some(bullet_decor()));
    let wrap = WrapMap::new(width).sync(content, &[line]);

    assert!(
        wrap.total_rows() > 1,
        "fixture must actually wrap for this test to be meaningful"
    );
    for seg in wrap.segments() {
        let content_cells = row_content_cells(content, seg);
        let decor_cells = seg.decor.as_ref().map(|d| d.cells).unwrap_or(0);
        assert!(
            content_cells + decor_cells <= width as usize,
            "segment content ({content_cells} cells) + decor ({decor_cells} cells) exceeds width {width}"
        );
    }
}

// ---------------------------------------------------------------------
// (ii) query outputs are byte-identical with and without decor, at a width
// wide enough that the decor reservation never changes the row structure.
// ---------------------------------------------------------------------

#[test]
fn query_outputs_agree_with_and_without_decor_when_the_row_structure_is_unchanged() {
    let content = "a short line of prose that comfortably fits one row";
    let width = 200u16;

    let decorated = one_span_line(content, Some(bullet_decor()));
    let plain = one_span_line(content, None);

    let wrap_decorated = WrapMap::new(width).sync(content, &[decorated]);
    let wrap_plain = WrapMap::new(width).sync(content, &[plain]);

    assert_eq!(wrap_decorated.total_rows(), 1);
    assert_eq!(wrap_plain.total_rows(), 1);

    for col in 0..=content.len() {
        let sp = SyntaxPoint { line: 0, col };
        let wp_d = wrap_decorated.syntax_to_wrap(sp);
        let wp_p = wrap_plain.syntax_to_wrap(sp);
        assert_eq!(wp_d, wp_p, "syntax_to_wrap diverged at col {col}");

        let back_d = wrap_decorated.wrap_to_syntax(content, wp_d);
        let back_p = wrap_plain.wrap_to_syntax(content, wp_p);
        assert_eq!(back_d, back_p, "wrap_to_syntax diverged at col {col}");

        let vis_d = wrap_decorated.visual_col(content, 0, col);
        let vis_p = wrap_plain.visual_col(content, 0, col);
        assert_eq!(vis_d, vis_p, "visual_col diverged at col {col}");

        let byte_d = wrap_decorated.byte_col_from_visual(content, 0, vis_d);
        let byte_p = wrap_plain.byte_col_from_visual(content, 0, vis_p);
        assert_eq!(byte_d, byte_p, "byte_col_from_visual diverged at col {col}");
    }
}

// ---------------------------------------------------------------------
// (iii) wrapped-quote continuation rows carry the bar.
// ---------------------------------------------------------------------

#[test]
fn wrapped_quote_continuation_rows_carry_the_bar() {
    let content = "one two three four five six seven eight nine ten eleven twelve";
    let width = 12u16;
    let line = one_span_line(content, Some(quote_bar_decor()));
    let wrap = WrapMap::new(width).sync(content, &[line]);

    assert!(wrap.total_rows() > 1, "fixture must actually wrap");
    for (i, seg) in wrap.segments().iter().enumerate() {
        assert!(seg.decor.is_some(), "row {i} lost its quote bar");
        let d = seg.decor.as_ref().expect("checked above");
        assert_eq!(
            d.pieces.first().map(|p| p.text.as_str()),
            Some("\u{258E}"),
            "row {i} must carry the bar on every row, first and continuation alike"
        );
    }
}

// ---------------------------------------------------------------------
// (iv) width 0 and width 1 degrade without panicking.
// ---------------------------------------------------------------------

#[test]
fn width_zero_and_width_one_degrade_without_panicking() {
    let content = "a decorated line with more than one word in it";
    for width in [0u16, 1u16] {
        let line = one_span_line(content, Some(bullet_decor()));
        let wrap = WrapMap::new(width).sync(content, &[line]);
        assert!(wrap.total_rows() >= 1);

        let hr = empty_line(Some(rule_decor(20)));
        let wrap_hr = WrapMap::new(width).sync("", &[hr]);
        assert!(wrap_hr.total_rows() >= 1);
    }
}

// ---------------------------------------------------------------------
// (v) an hr at width 10 carries a 10-cell rule decor.
// ---------------------------------------------------------------------

#[test]
fn hr_at_width_ten_carries_a_ten_cell_rule_decor() {
    let line = empty_line(Some(rule_decor(20)));
    let wrap = WrapMap::new(10).sync("", &[line]);

    assert_eq!(wrap.total_rows(), 1);
    let seg = &wrap.segments()[0];
    let d = seg.decor.as_ref().expect("hr must always carry its rule decor");
    assert_eq!(d.cells, 10);
    assert_eq!(
        d.pieces.iter().map(|p| p.text.chars().count()).sum::<usize>(),
        10
    );
}
