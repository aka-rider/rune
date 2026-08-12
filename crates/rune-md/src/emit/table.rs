//! `Block::Table`'s emit arm (plan WP2.S7, WP4) — split out of `walk.rs` on
//! its own (`walk.rs` was already over the file-size limit before this
//! function existed) since a table's layout selection and per-row dispatch
//! is a self-contained concern, distinct from the rest of the block/inline
//! tree walk.

use super::style::{table_header_scope, table_scope, verbatim_style};
use super::{EmitOut, hide_range, line_local, push_span_split_by_line};
use crate::element::table::TableM;
use crate::parse::line_at;
use crate::table::{CellSrc, extra_row_spans, layout, pivot, render, row_spans, wrapped};
use rune_core::assert_invariant;
use rune_syntax::ScopeId;
use rune_syntax::SyntaxSpan;
use rune_syntax::element::RevealState;
use rune_syntax::syntax::{RowBoundary, TableRole, TableRowInfo};

/// Renders a table's Grid, Wrapped, or Pivoted layout. `Revealed` shows raw
/// markdown, line by line, exactly like `Block::Verbatim` — `out.tables`
/// stays `None` for those lines, there is no rendered geometry to describe.
/// `Rendered` picks ONE layout for the WHOLE table (`table::layout::choose`,
/// keyed off `out.width` and the table's own natural/atomic column widths)
/// and renders every source line under it:
/// - Grid — a header/body content row via `table::layout::grid_row`, the
///   (comrak-absent, Gotcha 10) delimiter line via
///   `table::layout::separator_row`, real per-char buffer provenance
///   throughout.
/// - Wrapped — the same box shape at PROPORTIONALLY SHRUNK column widths
///   (`table::layout::constrain_widths`), each cell's own text word-wrapped
///   (`table::wrapped::wrap_cell`) across as many visual rows as the widest
///   cell in that row needs; every char is decorative (`buf = -1`) once
///   wrapping has reshuffled a cell's content.
/// - Pivoted — a header/separator line renders BLANK (suppressed); a body
///   line renders one `"  Label: Value"` row per column
///   (`table::pivot::pivot_rows`), preceded by a `─`-filled rule for every
///   record but the first. Label chars are decorative (Gotcha 4: the label
///   comes from the HEADER line, not this row's own bytes); value chars
///   keep their real offsets.
///
/// Every layout still tiles row 1 onto the source line's own byte range
/// exactly (`table::row_spans`, claimed whole through `EmitOut::claim_whole`
/// — a rendered row has no byte-for-byte relationship to its source, so a
/// refused claim is skipped rather than drawn over whatever already
/// occupies part of the line; the row's raw markdown then reaches the
/// display through `fill_gaps` instead). Deliberately NOT routed through
/// `push_span_split_by_line` itself (it only ever copies `content[range]`
/// verbatim) — this substitutes a wholly different string for the claimed
/// bytes, `push_task_checkbox`'s "substitutes visible content" shape, one
/// call per source line rather than one call per delimiter/content
/// sub-range. A Wrapped/Pivoted line's visual rows 2..N carry NO byte claim
/// at all: they become `TableRowInfo::extra_rows` via
/// `table::extra_row_spans`, never touching the emitted spans or
/// accounting, so a table line's visible-plus-hidden byte accounting stays
/// whole regardless of how many visual rows it expands to.
pub(super) fn emit_table(content: &str, starts: &[usize], t: &TableM, out: &mut EmitOut) {
    if t.sm.state() == RevealState::Revealed {
        for &line in &t.content_lines {
            push_span_split_by_line(
                content,
                starts,
                line,
                verbatim_style(),
                RevealState::Revealed,
                out,
            );
        }
        return;
    }

    let n_cols = t.aligns.len();
    let header_scope = table_header_scope();
    let body_scope = table_scope();

    // Every row's cells are rendered up front: `col_widths` is the max over
    // ALL rows, so no row can be laid out until every row's own cells are
    // known.
    let rendered_rows: Vec<Vec<render::RenderedCell>> = t
        .rows
        .iter()
        .map(|row| {
            let base = if row.is_header {
                header_scope
            } else {
                body_scope
            };
            row.cells
                .iter()
                .map(|c| render::render_cell(content, c, base))
                .collect()
        })
        .collect();
    let (natural_widths, min_widths) = layout::col_widths(&rendered_rows, n_cols);

    let avail = out.width as usize;
    let table_layout = layout::choose(&natural_widths, &min_widths, avail);

    // The ONE width vector this table is laid out at: Wrapped shrinks the
    // natural widths proportionally, Grid and Pivoted keep them. Every
    // rendered row AND the row info the border synthesizer reads must come
    // from this same vector — holding the shrunk and natural widths in two
    // variables is what let a Wrapped table emit content rows at one width
    // and borders at another.
    let layout_widths: Vec<usize> = if table_layout == layout::TableLayout::Wrapped {
        let frame_overhead = if n_cols > 0 { 4 * n_cols - 1 } else { 0 };
        let content_budget = avail.saturating_sub(frame_overhead);
        layout::constrain_widths(&natural_widths, &min_widths, content_budget)
    } else {
        natural_widths
    };

    // Pivoted-only bookkeeping: the header row's own rendered cells supply
    // every body row's LABELS; the first body row (found by document
    // order, not `sep_line` arithmetic — robust to a malformed or absent
    // delimiter line) never gets a leading separator rule.
    let header_cells: &[render::RenderedCell] = t
        .rows
        .iter()
        .zip(rendered_rows.iter())
        .find(|(r, _)| r.is_header)
        .map_or(&[], |(_, cells)| cells.as_slice());
    let first_body_line = t.rows.iter().find(|r| !r.is_header).map(|r| r.line);

    let total_lines = t.content_lines.len();
    for (i, &content_line) in t.content_lines.iter().enumerate() {
        let line = line_at(starts, content_line.start);
        let boundary = if total_lines <= 1 {
            RowBoundary::Only
        } else if i == 0 {
            RowBoundary::First
        } else if i == total_lines - 1 {
            RowBoundary::Last
        } else {
            RowBoundary::Middle
        };

        let found_row = t
            .rows
            .iter()
            .zip(rendered_rows.iter())
            .find(|(r, _)| r.line == line);
        let role_and_cells: Option<(TableRole, &[render::RenderedCell])> =
            if let Some((row, cells)) = found_row {
                let role = if row.is_header {
                    TableRole::Header
                } else {
                    TableRole::Body
                };
                Some((role, cells.as_slice()))
            } else if line == t.sep_line {
                Some((TableRole::Separator, &[]))
            } else {
                None
            };

        let Some((role, cells)) = role_and_cells else {
            // Neither a modeled row's line nor the derived separator line
            // — an unexpected gap within the table's own span (should not
            // occur for a well-formed table; degrade to raw text rather
            // than inventing content).
            push_span_split_by_line(
                content,
                starts,
                content_line,
                verbatim_style(),
                RevealState::Revealed,
                out,
            );
            continue;
        };

        let row1_runs: Vec<(String, Vec<CellSrc>, ScopeId)>;
        let extra_row_runs: Vec<Vec<(String, Vec<CellSrc>, ScopeId)>>;
        match table_layout {
            layout::TableLayout::Grid => {
                extra_row_runs = Vec::new();
                row1_runs = match role {
                    TableRole::Separator => layout::separator_row(&layout_widths),
                    TableRole::Header | TableRole::Body => {
                        let base = if role == TableRole::Header {
                            header_scope
                        } else {
                            body_scope
                        };
                        layout::grid_row(&layout_widths, &t.aligns, cells, base)
                    }
                };
            }
            layout::TableLayout::Wrapped => match role {
                TableRole::Separator => {
                    row1_runs = layout::separator_row(&layout_widths);
                    extra_row_runs = Vec::new();
                }
                TableRole::Header | TableRole::Body => {
                    let base = if role == TableRole::Header {
                        header_scope
                    } else {
                        body_scope
                    };
                    let wrapped_cells: Vec<Vec<String>> = (0..n_cols)
                        .map(|col| {
                            let text = cells.get(col).map_or("", |c| c.text.as_str());
                            let w = layout_widths.get(col).copied().unwrap_or(0);
                            wrapped::wrap_cell(text, w)
                        })
                        .collect();
                    let max_lines = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
                    let mut rows: Vec<Vec<(String, Vec<CellSrc>, ScopeId)>> = (0..max_lines)
                        .map(|k| wrapped::wrapped_row(&layout_widths, &wrapped_cells, k, base))
                        .collect();
                    row1_runs = if rows.is_empty() {
                        Vec::new()
                    } else {
                        rows.remove(0)
                    };
                    extra_row_runs = rows;
                }
            },
            layout::TableLayout::Pivoted => match role {
                TableRole::Header | TableRole::Separator => {
                    row1_runs = Vec::new();
                    extra_row_runs = Vec::new();
                }
                TableRole::Body => {
                    let include_separator = Some(line) != first_body_line;
                    let mut rows = pivot::pivot_rows(header_cells, cells, include_separator, avail);
                    row1_runs = if rows.is_empty() {
                        Vec::new()
                    } else {
                        rows.remove(0)
                    };
                    extra_row_runs = rows;
                }
            },
        }

        let line_start = content_line.start;
        let line_len = content_line.len();
        let spans = row_spans(line_start, line_len, &row1_runs);
        if spans.is_empty() {
            hide_range(content, starts, content_line, out);
        } else {
            match line_local(
                content.len(),
                starts,
                line,
                line_start..line_start + line_len,
            ) {
                Some(ll) => {
                    if let Ok(granted) = out.claim_whole(ll) {
                        granted.push_visible(spans);
                    }
                }
                None => assert_invariant!(false, || {
                    format!(
                        "table row on line {line}: rendered row range [{line_start},{}) escaped its own physical line bounds — producer bug",
                        line_start + line_len
                    )
                }),
            }
        }

        let extra_rows: Vec<Vec<SyntaxSpan>> = extra_row_runs
            .iter()
            .map(|runs| extra_row_spans(line_start, runs))
            .collect();

        if let Some(slot) = out.tables.get_mut(line) {
            *slot = Some(TableRowInfo {
                col_widths: layout_widths.clone(),
                role,
                boundary,
                extra_rows,
                boxed: table_layout != layout::TableLayout::Pivoted,
            });
        }
    }
}
