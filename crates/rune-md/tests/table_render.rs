//! WP2.S9: end-to-end Grid rendering through the real pipeline (parse ->
//! sync_cursors -> emit) — a header/separator/body row's exact rendered
//! text, tiling, equal display width across rows, revealed-vs-rendered
//! byte-verbatim behaviour, alignment, and the CJK column-width case WP6's
//! parity gate cannot cover (Gotcha/critique B7: Go's `cjk.md` fixture hits
//! an unfixable vendored-renderer TAB-padding defect, so this crate's own
//! width correctness is pinned here instead).
//!
//! Every "Rendered" assertion below uses `focused = false`: an unfocused
//! document forces every Decide-policy block Rendered regardless of cursor
//! position (`DocMachine::sync_cursors`'s `RevealGrant::ForceRendered`
//! root grant) — simpler than hunting for a cursor offset genuinely
//! outside a table whose every line IS the table.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;
use rune_md::emit::emit;
use rune_syntax::SyntaxSpan;
use rune_syntax::wrap::grapheme_width;
use unicode_segmentation::UnicodeSegmentation;

fn synced(content: &str, cursor_offset: usize, focused: bool) -> (Buffer, DocMachine) {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_focus(focused);
    doc.sync_content(&buf);
    let offset = cursor_offset.min(buf.len());
    let cursors = CursorSet::new(offset);
    doc.sync_cursors(&buf, &cursors);
    (buf, doc)
}

fn joined_line(lines: &[rune_syntax::SyntaxLine], line: usize, content: &str) -> String {
    lines
        .get(line)
        .map(|l| l.spans.iter().map(|s| s.text(content)).collect::<String>())
        .unwrap_or_default()
}

fn display_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

const NAME_AGE_TABLE: &str = "| Name | Age |\n| ---- | --- |\n| Alice | 30 |\n";

/// Pins the exact header-row text: column 0 ("Name"/"Alice") is width 5,
/// column 1 ("Age"/"30") is width 3 — `"│ Name  │ Age │"` (a trailing fill
/// space after "Name" to reach width 5, plus the one side-padding space
/// every column gets).
#[test]
fn header_row_renders_exact_grid_text() {
    let (buf, doc) = synced(NAME_AGE_TABLE, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "│ Name  │ Age │");
}

/// Pins the exact separator-row text. The plan's own worked value for this
/// row (`"├────────┼─────┤"`, 16 chars) does not tile: it puts 8 dashes in
/// the first segment where the header/body rows' own bar sits at column 8
/// (a 7-dash segment in the actual, geometry-consistent rendering), so the
/// delimiter row's `┼`/`┤` would land one column to the right of the header
/// row's own `│`s — measured wrong against the `Σw + 3n + 1` total every
/// other row in this table satisfies (confirmed: the plan's string is 16
/// chars, one longer than the header/body rows' own 15). The value pinned
/// here (`"├───────┼─────┤"`, 15 chars, 7 dashes then 5) is the one that
/// keeps every border character in the same visual column across all three
/// rows — see `bar_and_corner_columns_align_across_every_row` below.
#[test]
fn separator_row_renders_exact_grid_text() {
    let (buf, doc) = synced(NAME_AGE_TABLE, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 1, buf.content()), "├───────┼─────┤");
}

#[test]
fn body_row_renders_exact_grid_text() {
    let (buf, doc) = synced(NAME_AGE_TABLE, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 2, buf.content()), "│ Alice │ 30  │");
}

#[test]
fn bar_and_corner_columns_align_across_every_row() {
    let (buf, doc) = synced(NAME_AGE_TABLE, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let header = joined_line(&lines, 0, buf.content());
    let sep = joined_line(&lines, 1, buf.content());
    let body = joined_line(&lines, 2, buf.content());

    let border_cols = |s: &str| -> Vec<usize> {
        s.chars()
            .enumerate()
            .filter(|&(_, c)| matches!(c, '│' | '├' | '┼' | '┤'))
            .map(|(i, _)| i)
            .collect()
    };
    let hc = border_cols(&header);
    let sc = border_cols(&sep);
    let bc = border_cols(&body);
    assert_eq!(hc, sc, "separator borders must align with header borders");
    assert_eq!(hc, bc, "body borders must align with header borders");
}

#[test]
fn all_three_rendered_rows_share_the_same_display_width() {
    let (buf, doc) = synced(NAME_AGE_TABLE, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let w0 = display_width(&joined_line(&lines, 0, buf.content()));
    let w1 = display_width(&joined_line(&lines, 1, buf.content()));
    let w2 = display_width(&joined_line(&lines, 2, buf.content()));
    assert_eq!(w0, w1);
    assert_eq!(w1, w2);
}

/// Every rendered table line's span ranges must tile `[line_start,
/// line_end)` exactly — Gotcha 1: any unclaimed byte comes back through
/// `fill_gaps` as a spurious `Identical` span carrying raw markdown text.
#[test]
fn rendered_row_spans_tile_each_line_exactly() {
    let (buf, doc) = synced(NAME_AGE_TABLE, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    for (line, l) in lines.iter().enumerate().take(buf.line_count()) {
        let line_start = buf.line_start(line);
        let line_len = buf.line(line).len();
        let mut cursor = line_start;
        for span in &l.spans {
            let r = span.range();
            assert_eq!(
                r.start, cursor,
                "line {line}: gap or overlap in span tiling at byte {cursor}"
            );
            cursor = r.end;
        }
        assert_eq!(
            cursor,
            line_start + line_len,
            "line {line}: spans do not cover the whole line"
        );
    }
}

/// Cursor on the table's own body line (line 2) reveals the whole block as
/// a unit (plan architectural decision 5) — every line renders as raw,
/// byte-verbatim `Identical` spans, exactly like any other Decide-policy
/// block.
#[test]
fn cursor_inside_table_reveals_every_line_byte_verbatim() {
    let cursor = NAME_AGE_TABLE.rfind("Alice").expect("fixture has Alice");
    let (buf, doc) = synced(NAME_AGE_TABLE, cursor, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    for line in 0..3 {
        let l = &lines[line];
        assert!(
            l.spans
                .iter()
                .all(|s| matches!(s, SyntaxSpan::Identical { .. })),
            "line {line}: expected every span Identical (revealed), got {:?}",
            l.spans
        );
        let joined = joined_line(&lines, line, buf.content());
        assert_eq!(joined, buf.line(line), "line {line} not byte-verbatim");
    }
}

/// `:---:` centres a column's content within its width; `---:` right-aligns
/// it — verified against the same padded-content geometry the header/body
/// tests above pin (one side-padding space plus fill distributed by
/// `table::layout::push_padded_content`'s alignment rule).
#[test]
fn center_alignment_centres_short_content() {
    let content = "| c |\n| :-: |\n| xx |\n";
    let (buf, doc) = synced(content, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "│ c  │");
    assert_eq!(joined_line(&lines, 2, buf.content()), "│ xx │");
}

#[test]
fn right_alignment_right_aligns_short_content() {
    let content = "| r |\n| ---: |\n| xx |\n";
    let (buf, doc) = synced(content, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    assert_eq!(joined_line(&lines, 0, buf.content()), "│  r │");
    assert_eq!(joined_line(&lines, 2, buf.content()), "│ xx │");
}

/// CJK column-width case WP6's parity gate cannot cover (Go's own `cjk.md`
/// fixture hits an unfixable vendored-renderer TAB-padding defect,
/// `scripts/parity/grid.sh`'s own documented exclusion) — pinned as a unit
/// test instead. `世界` is two CJK (double-width) chars, so column 0's
/// computed width must be exactly 4 (measured in display cells via
/// `grapheme_width`), not `"世界".chars().count() == 2`.
#[test]
fn cjk_column_width_is_measured_in_display_cells_not_chars() {
    let content = "| 世界 | b |\n| --- | --- |\n| x | y |\n";
    let (buf, doc) = synced(content, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let info = lines[0].table.as_ref().expect("rendered table row");
    assert_eq!(info.col_widths[0], 4);
}
