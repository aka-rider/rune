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
use rune_md::table::layout::{TableLayout, choose};
use rune_syntax::SyntaxSpan;
use rune_syntax::wrap::WrapMap;
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

// ---------------------------------------------------------------------
// WP4: Wrapped and Pivoted layouts.
// ---------------------------------------------------------------------

/// A 65-char URL, a wide-but-word-short "Description" column, and a short
/// "Name" column — sized (worked out against `layout::choose`'s own
/// formulas) so the table's natural Grid width does not fit at 100 columns
/// but Wrapped viably does, and so it collapses all the way to Pivoted at
/// 20 columns. One row only, so `include_separator` is `false` and the
/// Pivoted case has exactly one record to check.
fn wrap_pivot_url() -> String {
    let url: String = format!("https://{}", "a".repeat(57));
    assert_eq!(url.chars().count(), 65, "fixture must stay a 65-char URL");
    format!(
        "| Name | Description | URL |\n| --- | --- | --- |\n| Alice | quick brown fox jumps over lazy dog | {url} |\n"
    )
}

/// WP4.S7: at width 100 this table doesn't fit Grid (natural width 115)
/// but fits Wrapped (verified against `choose`'s own thresholds) — the
/// 65-char URL column is atomic and gets its own full natural width, so
/// `wrap_cell` never touches it; the "Description" column (all short
/// words) wraps across more than one visual row instead. Every visual row
/// of the body line must still come out the same total display width, and
/// the URL must appear intact (as one contiguous substring) in whichever
/// visual row carries it.
#[test]
fn wrapped_layout_keeps_a_long_url_intact_with_equal_width_visual_rows() {
    let content = wrap_pivot_url();
    let (buf, doc) = synced(&content, 0, false);
    let width = 100u16;
    let (lines, _snap) = emit(buf.content(), doc.blocks(), width);

    // Sanity: this fixture really does pick Wrapped at this width, not
    // Grid or Pivoted — confirms the fixture's own arithmetic, not just
    // its rendered shape.
    let widths = vec![5usize, 35, 65];
    let min_widths = vec![5usize, 11, 65];
    assert_eq!(
        choose(&widths, &min_widths, width as usize),
        TableLayout::Wrapped
    );

    let body_line = 2;
    let info = lines[body_line].table.as_ref().expect("rendered table row");
    assert!(
        !info.extra_rows.is_empty(),
        "the wide Description cell must wrap into more than one visual row"
    );

    let row1_text = joined_line(&lines, body_line, buf.content());
    let row1_width = display_width(&row1_text);
    for extra in &info.extra_rows {
        let extra_text: String = extra.iter().map(|s| s.text(buf.content())).collect();
        assert_eq!(
            display_width(&extra_text),
            row1_width,
            "every visual row of a Wrapped table row must share the same display width"
        );
    }

    // The URL is atomic and fits its own column exactly — it must appear
    // whole, in exactly one visual row (never split across two).
    let url: String = format!("https://{}", "a".repeat(57));
    let mut rows_containing_url = 0usize;
    if row1_text.contains(&url) {
        rows_containing_url += 1;
    }
    for extra in &info.extra_rows {
        let extra_text: String = extra.iter().map(|s| s.text(buf.content())).collect();
        if extra_text.contains(&url) {
            rows_containing_url += 1;
        }
    }
    assert_eq!(
        rows_containing_url, 1,
        "the URL must appear intact exactly once"
    );
    assert!(
        !row1_text.contains('|'),
        "a rendered row never carries raw pipes"
    );
}

/// WP4.S7: the SAME table collapses to Pivoted at width 20 (verified
/// against `choose`'s own thresholds: every column is atomic-dominant once
/// the frame overhead eats most of the tiny content budget). The body row
/// becomes one `"  Label: Value"` row per column — `"Name: "` must appear,
/// and no `│` anywhere (Pivoted abandons the box shape entirely).
#[test]
fn pivoted_layout_renders_label_value_pairs_with_no_box_drawing() {
    let content = wrap_pivot_url();
    let (buf, doc) = synced(&content, 0, false);
    let width = 20u16;
    let (lines, _snap) = emit(buf.content(), doc.blocks(), width);

    let widths = vec![5usize, 35, 65];
    let min_widths = vec![5usize, 11, 65];
    assert_eq!(
        choose(&widths, &min_widths, width as usize),
        TableLayout::Pivoted
    );

    let body_line = 2;
    let info = lines[body_line].table.as_ref().expect("rendered table row");

    let mut all_text = joined_line(&lines, body_line, buf.content());
    for extra in &info.extra_rows {
        all_text.push_str(
            &extra
                .iter()
                .map(|s| s.text(buf.content()))
                .collect::<String>(),
        );
    }
    assert!(
        all_text.contains("Name: "),
        "expected a Name label:value pair"
    );
    assert!(!all_text.contains('│'), "Pivoted never draws a box");

    // Header and separator lines are suppressed to blank under Pivoted.
    assert!(joined_line(&lines, 0, buf.content()).is_empty());
    assert!(joined_line(&lines, 1, buf.content()).is_empty());
}

#[test]
fn choose_selects_grid_unconditionally_when_avail_is_zero() {
    assert_eq!(choose(&[50, 50, 50], &[10, 10, 10], 0), TableLayout::Grid);
}

/// WP4.S7: for a Wrapped source line, the wrap pass must yield exactly
/// `1 + extra_rows.len()` segments for that line, each one's `start_col`
/// the running sum of the PREVIOUS rows' own visible lengths — the
/// existing `wrap_table_line` machinery this package builds on, exercised
/// end to end through a real Wrapped table for the first time.
#[test]
fn wrapped_line_produces_one_wrap_segment_per_visual_row_at_running_start_cols() {
    let content = wrap_pivot_url();
    let (buf, doc) = synced(&content, 0, false);
    let width = 100u16;
    let (lines, _snap) = emit(buf.content(), doc.blocks(), width);

    let body_line = 2;
    let info = lines[body_line].table.as_ref().expect("rendered table row");
    let expected_segments = 1 + info.extra_rows.len();

    let wrap = WrapMap::new(width).sync(buf.content(), &lines);
    let segs: Vec<_> = wrap
        .segments()
        .iter()
        .filter(|s| s.model_line == body_line)
        .collect();
    assert_eq!(segs.len(), expected_segments);

    let mut expected_start = 0usize;
    for seg in &segs {
        assert_eq!(seg.start_col, expected_start);
        let seg_len: usize = seg.spans.iter().map(|s| s.text(buf.content()).len()).sum();
        expected_start += seg_len;
    }
}

/// A Pivoted table draws no box, so the display pass must not synthesise
/// `┌┬┐`/`├┼┤`/`└┴┘` rows around it. Asserted at the DISPLAY level rather
/// than on spans: border rows never appear in a `SyntaxLine`'s spans at
/// all, so a span-level "no box drawing" check passes whether or not the
/// expansion pass is doing the right thing.
#[test]
fn pivoted_table_gets_no_synthetic_border_rows_in_the_display_snapshot() {
    let content = wrap_pivot_url();
    let (buf, doc) = synced(&content, 0, false);

    let grid = display_rows_at(&buf, &doc, 200);
    let pivot = display_rows_at(&buf, &doc, 20);

    let boxy = |rows: &[String]| {
        rows.iter()
            .filter(|r| r.starts_with('┌') || r.starts_with('└') || r.starts_with('├'))
            .count()
    };

    assert!(
        boxy(&grid) > 0,
        "a Grid table must still get border rows: {grid:#?}"
    );
    assert_eq!(
        boxy(&pivot),
        0,
        "a Pivoted table must get no synthesised border rows: {pivot:#?}"
    );
}

fn display_rows_at(buf: &Buffer, doc: &DocMachine, width: u16) -> Vec<String> {
    let (lines, _snap) = emit(buf.content(), doc.blocks(), width);
    let wrap = WrapMap::new(width).sync(buf.content(), &lines);
    let display = rune_md::snapshot::DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
    display
        .rows()
        .iter()
        .map(|r| {
            r.spans
                .iter()
                .map(|s| s.text(buf.content()))
                .collect::<String>()
        })
        .collect()
}

/// Regression (found via WP6's `parity-grid` gate, `tables-narrow.md`):
/// `wrap_table_line` used to stamp the SAME `TableRowInfo::boundary` onto
/// every visual sub-row of a Wrapped body line, so
/// `DisplaySnapshot::expand_tables` synthesised a `└┴┘` bottom border
/// after EVERY visual row instead of only the last one; and `emit_table`
/// stored the Grid layout's natural (unshrunk) `col_widths` in
/// `TableRowInfo` even when Wrapped was the layout actually chosen, so any
/// synthesised border ended up wider than the content rows it bordered.
/// Both are asserted here at a width where the body row wraps into more
/// than one visual row.
#[test]
fn wrapped_table_gets_exactly_one_top_and_one_bottom_border_at_the_constrained_width() {
    let content = wrap_pivot_url();
    let (buf, doc) = synced(&content, 0, false);
    let width = 100u16;
    let rows = display_rows_at(&buf, &doc, width);

    let top: Vec<&String> = rows.iter().filter(|r| r.starts_with('┌')).collect();
    let bottom: Vec<&String> = rows.iter().filter(|r| r.starts_with('└')).collect();
    assert_eq!(top.len(), 1, "exactly one top border row: {rows:#?}");
    assert_eq!(
        bottom.len(),
        1,
        "exactly one bottom border row, not one per visual sub-row of the wrapped body line: {rows:#?}"
    );

    // Every content row's own display width must equal the border rows'
    // width — a border sized off the Grid layout's natural widths would be
    // wider than the Wrapped content it borders.
    let content_width = rows
        .iter()
        .find(|r| {
            !r.starts_with('┌') && !r.starts_with('└') && !r.starts_with('├') && !r.is_empty()
        })
        .map(|r| display_width(r))
        .expect("at least one content row");
    for border in top.iter().chain(bottom.iter()) {
        assert_eq!(
            display_width(border),
            content_width,
            "a synthesised border must match the Wrapped layout's own constrained width, not Grid's natural width"
        );
    }
}
