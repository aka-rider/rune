//! Grid geometry (WP2.S6): column widths and the box-drawn rows that tile a
//! table source line exactly, fed to [`super::row_spans`] for the final
//! byte-tiling. A row is built as one FLAT sequence of `(char, buf, scope)`
//! triples covering the whole rendered row left to right, then grouped
//! into maximal same-scope runs — the shape `row_spans` consumes. Grouping
//! at the end (rather than hand-assembling per-column runs) means two
//! adjacent decorative pieces that happen to share a scope merge for free.

use unicode_segmentation::UnicodeSegmentation;

use crate::element::table::TableAlign;
use crate::emit::style;
use rune_syntax::ScopeId;
use rune_syntax::wrap::grapheme_width;

use super::CellSrc;
use super::render::RenderedCell;

/// Total display width of `text` — the sum of each grapheme cluster's
/// `grapheme_width` (CONSTITUTION §1.5: widths are terminal cells, never
/// `.len()`'s bytes nor `.chars().count()`'s scalar values — a ZWJ/skin-tone
/// emoji cluster is one cell-width unit, not one-per-`char`).
pub(crate) fn display_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

/// Per column, the max rendered display width over every row's own cell
/// (`rows`, one `Vec<RenderedCell>` per non-separator row — the delimiter
/// row has no `RenderedCell`s of its own to contribute, Gotcha 10), sized
/// to `n_cols`: a row shorter than `n_cols` contributes 0 to the columns it
/// doesn't reach rather than panicking.
pub fn col_widths(rows: &[Vec<RenderedCell>], n_cols: usize) -> Vec<usize> {
    let mut widths = vec![0usize; n_cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(display_width(&cell.text));
            }
        }
    }
    widths
}

/// One flat visible char plus its provenance — the intermediate shape
/// [`grid_row`]/[`separator_row`] build before grouping into the
/// `(String, Vec<CellSrc>, ScopeId)` runs `row_spans` consumes.
struct FlatChar {
    ch: char,
    buf: i64,
    scope: ScopeId,
}

/// Groups a flat char sequence into maximal contiguous same-scope runs —
/// the shared tail of [`grid_row`] and [`separator_row`], so both build
/// their row as one flat pass and let run boundaries fall out of the scope
/// changes rather than being hand-tracked per column.
fn group_runs(flat: &[FlatChar]) -> Vec<(String, Vec<CellSrc>, ScopeId)> {
    let mut runs: Vec<(String, Vec<CellSrc>, ScopeId)> = Vec::new();
    let mut cur_text = String::new();
    let mut cur_src: Vec<CellSrc> = Vec::new();
    let mut cur_scope: Option<ScopeId> = None;

    for fc in flat {
        if cur_scope != Some(fc.scope) {
            if let Some(scope) = cur_scope.take() {
                runs.push((
                    std::mem::take(&mut cur_text),
                    std::mem::take(&mut cur_src),
                    scope,
                ));
            }
            cur_scope = Some(fc.scope);
        }
        cur_text.push(fc.ch);
        cur_src.push(CellSrc {
            buf: fc.buf,
            scope: fc.scope,
        });
    }
    if let Some(scope) = cur_scope {
        runs.push((cur_text, cur_src, scope));
    }
    runs
}

/// Pushes one column's content slot, padded to `w` per `align`: `None`/
/// `Left` pads on the right, `Right` pads on the left, `Center` splits the
/// fill (the shorter half first, matching Go's own `(w-content)/2` floor-
/// division split). Padding chars are decorative (`buf = -1`, the row's own
/// role scope); the cell's own chars keep whatever [`super::render::render_cell`]
/// resolved for them.
fn push_padded_content(
    flat: &mut Vec<FlatChar>,
    cell: &RenderedCell,
    w: usize,
    align: TableAlign,
    role_scope: ScopeId,
) {
    let content_w = display_width(&cell.text);
    let fill = w.saturating_sub(content_w);
    let (left_fill, right_fill) = match align {
        TableAlign::Right => (fill, 0),
        TableAlign::Center => (fill / 2, fill - fill / 2),
        TableAlign::None | TableAlign::Left => (0, fill),
    };
    for _ in 0..left_fill {
        flat.push(FlatChar {
            ch: ' ',
            buf: -1,
            scope: role_scope,
        });
    }
    for (ch, src) in cell.text.chars().zip(cell.src.iter()) {
        flat.push(FlatChar {
            ch,
            buf: src.buf,
            scope: src.scope,
        });
    }
    for _ in 0..right_fill {
        flat.push(FlatChar {
            ch: ' ',
            buf: -1,
            scope: role_scope,
        });
    }
}

/// One Grid content row (header or body): `│` opens column 0 and closes
/// every column (`n + 1` bars total, row width `Σw + 3n + 1`), one padding
/// space each side of every column's content slot. Bars carry `buf = -1`
/// and `markup.table.border`; padding carries `buf = -1` and `role_scope`
/// (the row's own header/body scope) — DIFFERENT scopes, so a bar and its
/// adjacent padding space never merge into one run even though they sit
/// next to each other.
pub fn grid_row(
    widths: &[usize],
    aligns: &[TableAlign],
    cells: &[RenderedCell],
    role_scope: ScopeId,
) -> Vec<(String, Vec<CellSrc>, ScopeId)> {
    let border = style::table_border_scope();
    let empty = RenderedCell {
        text: String::new(),
        src: Vec::new(),
    };
    let mut flat: Vec<FlatChar> = Vec::new();
    flat.push(FlatChar {
        ch: '│',
        buf: -1,
        scope: border,
    });
    for (i, &w) in widths.iter().enumerate() {
        let align = aligns.get(i).copied().unwrap_or(TableAlign::None);
        let cell = cells.get(i).unwrap_or(&empty);
        flat.push(FlatChar {
            ch: ' ',
            buf: -1,
            scope: role_scope,
        });
        push_padded_content(&mut flat, cell, w, align, role_scope);
        flat.push(FlatChar {
            ch: ' ',
            buf: -1,
            scope: role_scope,
        });
        flat.push(FlatChar {
            ch: '│',
            buf: -1,
            scope: border,
        });
    }
    group_runs(&flat)
}

/// The three border-row glyph sets a table ever draws: the Grid-replacing
/// delimiter row (`Middle`, `├┼┤`) today; the outer top/bottom borders
/// (`Top`/`Bottom`, `┌┬┐`/`└┴┘`) a later package (`DisplaySnapshot`'s
/// synthesised rows) synthesises around the whole table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderKind {
    Top,
    Bottom,
    Middle,
}

/// `w + 2` dash-fills per column (matching a content row's `Σw + 3n + 1`
/// total exactly: `1 + Σ(w+2) + (n-1) = Σw + 3n + 1`), corners per `kind`.
/// Plain `String` — a border-only row (no cell content, no buffer
/// provenance beyond "decorative") doesn't need `render_cell`'s per-char
/// `CellSrc`; callers that DO need that (`separator_row`, replacing the
/// source delimiter LINE and therefore needing a byte-tiled span) wrap this
/// string's chars in `buf = -1` themselves.
pub fn border_row(widths: &[usize], kind: BorderKind) -> String {
    let (left, mid, right) = match kind {
        BorderKind::Top => ('┌', '┬', '┐'),
        BorderKind::Bottom => ('└', '┴', '┘'),
        BorderKind::Middle => ('├', '┼', '┤'),
    };
    let mut s = String::new();
    s.push(left);
    for (i, &w) in widths.iter().enumerate() {
        if i > 0 {
            s.push(mid);
        }
        for _ in 0..(w + 2) {
            s.push('─');
        }
    }
    s.push(right);
    s
}

/// The Grid layout's replacement for the source `|---|---|` delimiter line
/// — one run, every char decorative (`buf = -1`) and scoped
/// `markup.table.separator` (distinct from a content row's
/// `markup.table.border`, Gotcha/plan WP2.S6).
pub fn separator_row(widths: &[usize]) -> Vec<(String, Vec<CellSrc>, ScopeId)> {
    let text = border_row(widths, BorderKind::Middle);
    let scope = style::table_separator_scope();
    let src: Vec<CellSrc> = text.chars().map(|_| CellSrc { buf: -1, scope }).collect();
    vec![(text, src, scope)]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn rendered(text: &str, start_buf: i64, scope: ScopeId) -> RenderedCell {
        let src = (0..text.chars().count() as i64)
            .map(|i| CellSrc {
                buf: start_buf + i,
                scope,
            })
            .collect();
        RenderedCell {
            text: text.to_string(),
            src,
        }
    }

    #[test]
    fn grid_row_pads_left_aligned_content_to_column_width() {
        let widths = vec![5, 3];
        let aligns = vec![TableAlign::None, TableAlign::None];
        let cells = vec![
            rendered("Name", 2, ScopeId(9)),
            rendered("Age", 9, ScopeId(9)),
        ];
        let runs = grid_row(&widths, &aligns, &cells, ScopeId(9));
        let text: String = runs.iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(text, "│ Name  │ Age │");
    }

    #[test]
    fn separator_row_matches_grid_rows_total_width() {
        let widths = vec![5, 3];
        let grid_text: String = grid_row(
            &widths,
            &[TableAlign::None, TableAlign::None],
            &[],
            ScopeId(1),
        )
        .iter()
        .map(|(t, _, _)| t.as_str())
        .collect::<String>();
        let sep = separator_row(&widths);
        let sep_text = &sep[0].0;
        assert_eq!(sep_text.chars().count(), grid_text.chars().count());
        // Bars/corners must land at the SAME visual column in both rows.
        let bar_cols: Vec<usize> = grid_text
            .chars()
            .enumerate()
            .filter(|&(_, c)| c == '│')
            .map(|(i, _)| i)
            .collect();
        let corner_cols: Vec<usize> = sep_text
            .chars()
            .enumerate()
            .filter(|&(_, c)| matches!(c, '├' | '┼' | '┤'))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(bar_cols, corner_cols);
    }
}
