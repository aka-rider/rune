//! WP2.S9: end-to-end Grid rendering through the real pipeline (parse ->
//! sync_cursors -> emit) — a header/separator/body row's exact rendered
//! text, tiling, equal display width across rows, revealed-vs-rendered
//! byte-verbatim behaviour, alignment, and the CJK column-width case WP6's
//! parity gate cannot cover (Gotcha/critique B7: Go's `cjk.md` fixture hits
//! an unfixable vendored-renderer TAB-padding defect, so this crate's own
//! width correctness is pinned here instead). Split from the combined
//! `table_render` file (§1.6) into per-layout groups — this one is Grid.
//!
//! Every "Rendered" assertion below uses `focused = false`: an unfocused
//! document forces every Decide-policy block Rendered regardless of cursor
//! position (`DocMachine::sync_cursors`'s `RevealGrant::ForceRendered`
//! root grant) — simpler than hunting for a cursor offset genuinely
//! outside a table whose every line IS the table.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod table_render_common;

use rune_md::emit::emit;
use rune_md::table::layout::{TableLayout, choose};
use rune_syntax::SyntaxSpan;
use table_render_common::{display_rows_at, joined_line, per_span_display_width, synced};

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
    let w0 = per_span_display_width(&lines, 0, buf.content());
    let w1 = per_span_display_width(&lines, 1, buf.content());
    let w2 = per_span_display_width(&lines, 2, buf.content());
    assert_eq!(w0, w1);
    assert_eq!(w1, w2);
}

/// TABLE-ROW-WIDTH root cause (WP9.S1/S2): a grapheme cluster straddling a
/// span boundary inside a cell (a ZWJ-joined emoji pair split by emphasis
/// markup) must measure and render the SAME width. Joined-text measurement
/// re-fuses the pair into one cluster across the boundary (GB11 joins a ZWJ
/// to a following pictograph unconditionally, span boundary or not) and
/// undercounts; the renderer can never do that — each span's text is
/// grapheme-segmented on its own, so the pair renders as two separate
/// clusters. A CJK cell sits in the other column so the fixture also pins
/// the (already-correct) per-grapheme, not per-`char`, measurement for wide
/// scalar values. This fails against the pre-fix `col_widths` (measures the
/// cell's joined text in one grapheme pass) with a torn box: the content
/// row renders wider than the border row it was sized against.
#[test]
fn zwj_family_split_by_emphasis_and_cjk_row_widths_agree() {
    // The emphasis boundary falls right before the ZWJ (`👨` plain, `*` opens
    // emphasis, then a LONE `\u{200d}` immediately starts the emphasised
    // run): the renderer grapheme-segments each of those two runs
    // independently, so the ZWJ (no preceding char in its OWN run) can never
    // join to `👨` — it stands as its own single-width cluster, then `👩` as
    // a second, separate cluster (2 + 1 + 2 = 5 cells). Joined-text
    // measurement instead re-fuses `👨` + the ZWJ + `👩` into ONE cluster
    // across the run boundary (UAX #29 GB9/GB11 join a ZWJ unconditionally
    // to whatever precedes it when the text is walked as one string) and
    // undercounts to 2.
    let content = "| 👨*\u{200d}👩* | 世界 |\n| --- | --- |\n| x | y |\n";
    let (buf, doc) = synced(content, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    let w0 = per_span_display_width(&lines, 0, buf.content());
    let w1 = per_span_display_width(&lines, 1, buf.content());
    let w2 = per_span_display_width(&lines, 2, buf.content());
    assert_eq!(
        w0, w1,
        "separator row must match header row's true rendered width"
    );
    assert_eq!(
        w1, w2,
        "body row must match header row's true rendered width"
    );

    let info = lines[0].table.as_ref().expect("rendered table row");
    assert_eq!(info.col_widths[0], 5);
}

/// Every rendered table line's span ranges must tile `[line_start,
/// line_end)` exactly — Gotcha 1: any unclaimed byte comes back through
/// `fill_gaps` as a spurious `Identical` span carrying raw markdown text.
#[test]
fn rendered_row_spans_tile_each_line_exactly() {
    let (buf, doc) = synced(NAME_AGE_TABLE, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    for (line, l) in lines.iter().enumerate().take(buf.line_count()) {
        let line_start = buf
            .line_start(line)
            .expect("line is in-range: iterating up to buf.line_count()");
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

#[test]
fn choose_selects_grid_unconditionally_when_avail_is_zero() {
    assert_eq!(choose(&[50, 50, 50], &[10, 10, 10], 0), TableLayout::Grid);
}

/// A table whose range starts mid-line (leading whitespace before the
/// header) must degrade to raw passthrough, not render. comrak reports the
/// cell sourcepos of every later row shifted one byte right in that case,
/// so rendering it drops the first character of every body cell and leaks
/// the skipped byte back as raw text — displaying the user's words wrongly.
/// Raw markdown is the correct fallback (§1.3).
#[test]
fn a_table_starting_mid_line_degrades_to_raw_text_instead_of_rendering_wrong() {
    let content = " Name | Age |\n| :--- | ---: |\n| Alice | 30 |\n";
    let (buf, doc) = synced(content, 0, false);
    let rows = display_rows_at(&buf, &doc, 80);

    for (i, r) in rows.iter().take(3).enumerate() {
        assert_eq!(
            r,
            buf.line(i),
            "line {i} must be byte-verbatim raw markdown"
        );
    }
    assert!(
        !rows
            .iter()
            .any(|r| r.contains('\u{2502}') || r.contains('\u{250c}')),
        "no box drawing may be rendered for a mid-line table: {rows:#?}"
    );

    // The same table with its leading pipe restored still renders normally,
    // so the guard is narrow rather than disabling tables wholesale.
    let ok = "| Name | Age |\n| :--- | ---: |\n| Alice | 30 |\n";
    let (buf2, doc2) = synced(ok, 0, false);
    let rows2 = display_rows_at(&buf2, &doc2, 80);
    assert!(
        rows2.iter().any(|r| r.starts_with('\u{250c}')),
        "{rows2:#?}"
    );
}
