//! Pivoted layout (WP4.S5): the last-resort layout for a table too wide to
//! render as any box at all — one BODY row becomes `n_cols` `"Label: Value"`
//! lines, no `│` anywhere. Split out of `layout.rs` on its own —
//! Pivoted's row builder shares `layout`'s
//! `FlatChar`/`group_runs` plumbing but is otherwise a distinct concern
//! from Grid geometry.

use super::CellSrc;
use super::layout::{FlatChar, group_runs};
use super::render::RenderedCell;
use crate::emit::style;
use rune_syntax::ScopeId;

/// Renders one BODY table row as `n_cols` `"  Label: Value"` lines (header/
/// separator rows are suppressed to nothing by the caller — this function
/// only ever sees a body row), with a `─`-filled horizontal rule inserted
/// BEFORE the whole group when `include_separator` (every record but the
/// first). No `│` anywhere: Pivoted abandons the box shape entirely.
///
/// Label characters carry `buf = -1` (Gotcha 4: the `CELL-ORDER` fuzz
/// invariant forbids a row's non-negative `buf_offset`s from decreasing,
/// and a label comes from the HEADER line while `body_cells` comes from
/// THIS row's own line — mixing them would go backwards; `-1` is also the
/// honest answer, since a label isn't this row's own byte at all). Value
/// characters keep whatever real buffer offset `render_cell` resolved for
/// them — unlike Wrapped, which never keeps a real per-char mapping,
/// Pivoted preserves it verbatim.
///
/// Returns one `Vec` of runs per VISUAL row, top to bottom (the separator
/// rule first, if included, then one row per column) — the caller takes
/// the first entry as row 1 (tiled onto the source line via
/// `table::row_spans`) and the rest as `TableRowInfo::extra_rows` (via
/// `table::extra_row_spans`, Gotcha 2: they claim no bytes).
pub fn pivot_rows(
    header_cells: &[RenderedCell],
    body_cells: &[RenderedCell],
    include_separator: bool,
    sep_width: usize,
) -> Vec<Vec<(String, Vec<CellSrc>, ScopeId)>> {
    let label_scope = style::table_header_scope();
    let value_scope = style::table_scope();
    let sep_scope = style::table_separator_scope();

    let mut rows: Vec<Vec<(String, Vec<CellSrc>, ScopeId)>> = Vec::new();

    if include_separator {
        let mut flat = Vec::new();
        for _ in 0..sep_width.max(1) {
            flat.push(FlatChar {
                ch: '─',
                buf: -1,
                scope: sep_scope,
            });
        }
        rows.push(group_runs(&flat));
    }

    for (i, body_cell) in body_cells.iter().enumerate() {
        let label_text = header_cells
            .get(i)
            .map(|c| c.text.as_str())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Col{}", i + 1));

        let mut flat: Vec<FlatChar> = Vec::new();
        for ch in "  ".chars() {
            flat.push(FlatChar {
                ch,
                buf: -1,
                scope: label_scope,
            });
        }
        for ch in label_text.chars() {
            flat.push(FlatChar {
                ch,
                buf: -1,
                scope: label_scope,
            });
        }
        for ch in ": ".chars() {
            flat.push(FlatChar {
                ch,
                buf: -1,
                scope: value_scope,
            });
        }
        for (ch, src) in body_cell.text.chars().zip(body_cell.src.iter()) {
            flat.push(FlatChar {
                ch,
                buf: src.buf,
                scope: value_scope,
            });
        }
        rows.push(group_runs(&flat));
    }

    rows
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn rendered(text: &str, start_buf: i64) -> RenderedCell {
        let src = (0..text.chars().count() as i64)
            .map(|i| CellSrc {
                buf: start_buf + i,
                scope: ScopeId(0),
            })
            .collect();
        RenderedCell {
            text: text.to_string(),
            src,
        }
    }

    #[test]
    fn first_record_has_no_leading_separator() {
        let header = vec![rendered("Name", -1)];
        let body = vec![rendered("Alice", 10)];
        let rows = pivot_rows(&header, &body, false, 20);
        assert_eq!(rows.len(), 1, "no separator row for the first record");
        let text: String = rows[0].iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(text, "  Name: Alice");
        assert!(!text.contains('│'));
    }

    #[test]
    fn later_record_gets_a_leading_rule_of_the_requested_width() {
        let header = vec![rendered("Name", -1)];
        let body = vec![rendered("Bob", 10)];
        let rows = pivot_rows(&header, &body, true, 8);
        assert_eq!(rows.len(), 2, "one rule row, then one label:value row");
        let rule: String = rows[0].iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(rule, "─".repeat(8));
    }

    #[test]
    fn label_characters_are_decorative_value_characters_keep_real_offsets() {
        let header = vec![rendered("Name", -1)];
        let body = vec![rendered("Alice", 10)];
        let rows = pivot_rows(&header, &body, false, 20);
        // The whole "  Name: " prefix (label) must be buf=-1; "Alice"
        // (value) keeps the real offsets `rendered` assigned it.
        let mut seen_real = false;
        for (text, src, _) in &rows[0] {
            for (ch, s) in text.chars().zip(src.iter()) {
                if ch.is_alphabetic() && "Alice".contains(ch) && s.buf >= 0 {
                    seen_real = true;
                }
            }
        }
        assert!(seen_real, "value chars must keep a real buf offset");
    }

    #[test]
    fn missing_header_label_falls_back_to_col_n() {
        let body = vec![rendered("x", 0)];
        let rows = pivot_rows(&[], &body, false, 20);
        let text: String = rows[0].iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(text, "  Col1: x");
    }
}
