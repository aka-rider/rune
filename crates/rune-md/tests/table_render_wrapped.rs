//! WP4: end-to-end Wrapped-layout table rendering through the real
//! pipeline (parse -> sync_cursors -> emit). Split from the combined
//! `table_render` file into per-layout groups — this one is
//! Wrapped.
//!
//! Every "Rendered" assertion below uses `focused = false`: an unfocused
//! document forces every Decide-policy block Rendered regardless of cursor
//! position (`DocMachine::sync_cursors`'s `RevealGrant::ForceRendered`
//! root grant) — simpler than hunting for a cursor offset genuinely
//! outside a table whose every line IS the table.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod table_render_common;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;
use rune_md::emit::emit;
use rune_md::table::layout::{TableLayout, choose};
use rune_syntax::SyntaxSpan;
use rune_syntax::scope::scope_table;
use rune_syntax::wrap::WrapMap;
use table_render_common::{display_rows_at, display_width, joined_line, synced, wrap_pivot_url};

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

/// Regression found via screen-capture comparison of a narrow-table
/// fixture: `wrap_table_line` used to stamp the SAME `TableRowInfo::boundary` onto
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

/// A Wrapped table's synthesised borders are derived from `TableRowInfo::
/// col_widths`, while its content rows are laid out at the proportionally
/// shrunk widths. Those were once two separate vectors and the row info got
/// the natural ones, so a table narrower than its content drew an 84-cell
/// border around 38-cell rows. They are one vector now; this pins that.
#[test]
fn a_wrapped_table_borders_at_the_width_it_lays_its_content_out_at() {
    let content = "| Alpha | Beta |\n| --- | --- |\n\
                   | the quick brown fox jumps over the lazy dog | a second rather long cell of prose |\n";
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_reveal_mode(false.into());
    doc.sync_content(&buf);
    doc.set_width(40);
    doc.sync_cursors(&buf, &CursorSet::new(0));

    let (lines, _) = emit(content, doc.blocks(), 40);

    let infos: Vec<usize> = lines
        .iter()
        .filter_map(|l| l.table.as_ref())
        .map(|t| t.col_widths.iter().sum::<usize>() + 3 * t.col_widths.len() + 1)
        .collect();
    assert!(!infos.is_empty(), "the fixture must produce table row info");

    for (line, _) in lines.iter().enumerate().filter(|(_, l)| l.table.is_some()) {
        let text = joined_line(&lines, line, content);
        let rendered = display_width(&text);
        let border = infos[0];
        assert_eq!(
            rendered, border,
            "row {line} renders {rendered} cells but its border is built at {border}: {text:?}"
        );
    }
}

/// `emit_table`'s Wrapped-only `frame_overhead = 4 * n_cols - 1` (n_cols
/// is 3 here) feeds `content_budget = avail - frame_overhead`, which
/// `constrain_widths` then stretches proportionally: get the `4`, the
/// `n_cols`, or the `- 1` wrong and the "Description" column's stretched
/// width (19, worked out by hand against `constrain_widths`'s own
/// proportional-split formula) comes out different — the URL and Name
/// columns are already at their own floor/atomic width and can't stretch,
/// so any budget error lands entirely on this one column.
#[test]
fn wrapped_frame_overhead_uses_four_times_column_count_minus_one() {
    let content = wrap_pivot_url();
    let (buf, doc) = synced(&content, 0, false);
    let width = 100u16;
    let (lines, _snap) = emit(buf.content(), doc.blocks(), width);

    let info = lines[2].table.as_ref().expect("rendered table row");
    assert_eq!(info.col_widths, vec![5, 19, 65]);
}

/// Same role-scope pin as Grid's own (`table_render_grid.rs`), but for the
/// Wrapped row builder's own `role == Header` check.
#[test]
fn wrapped_row_chars_carry_their_own_rows_role_scope_not_the_others() {
    let table = scope_table();
    let header_scope = table
        .resolve("markup.table.header")
        .expect("markup.table.header is registered");
    let body_scope = table
        .resolve("markup.table")
        .expect("markup.table is registered");

    let content = wrap_pivot_url();
    let (buf, doc) = synced(&content, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 100);

    let header_scopes: Vec<_> = lines[0].spans.iter().map(SyntaxSpan::scope).collect();
    let body_scopes: Vec<_> = lines[2].spans.iter().map(SyntaxSpan::scope).collect();
    assert!(
        header_scopes.iter().all(|&s| s != body_scope),
        "header row must never carry the body role scope: {header_scopes:?}"
    );
    assert!(
        body_scopes.iter().all(|&s| s != header_scope),
        "body row must never carry the header role scope: {body_scopes:?}"
    );
}
