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

/// A cell's rendered display width, measured the SAME way [`grid_row`]
/// renders it: group the cell's own chars into [`group_runs`]' maximal
/// same-scope runs FIRST (a run never straddles a scope change, exactly
/// what the row builder groups into spans), then grapheme-segment and sum
/// EACH run's own width independently, rather than grapheme-segmenting the
/// cell's joined text in one pass. The two only disagree when a grapheme
/// cluster (e.g. a ZWJ-joined emoji family) straddles a scope change inside
/// the cell (part plain, part emphasised) — joined-text segmentation fuses
/// it into one cluster, but the row builder can never do that: `group_runs`
/// already split the run there, so each half is grapheme-segmented on its
/// own and renders as separate, wider clusters. Measuring per-run here means
/// the width this function reports is the width the row actually renders,
/// by construction, instead of by coincidence.
pub(crate) fn cell_display_width(cell: &RenderedCell) -> usize {
    let flat: Vec<FlatChar> = cell
        .text
        .chars()
        .zip(cell.src.iter())
        .map(|(ch, src)| FlatChar {
            ch,
            buf: src.buf,
            scope: src.scope,
        })
        .collect();
    group_runs(&flat)
        .iter()
        .map(|(text, _, _)| display_width(text))
        .sum()
}

/// Per column, the max rendered display width over every row's own cell
/// (`rows`, one `Vec<RenderedCell>` per non-separator row — the delimiter
/// row has no `RenderedCell`s of its own to contribute, Gotcha 10), sized
/// to `n_cols`: a row shorter than `n_cols` contributes 0 to the columns it
/// doesn't reach rather than panicking. Also returns each column's own
/// MINIMUM width — the longest atomic (never-broken) unit any row's cell
/// contains — which Wrapped layout's selector and column-shrinking both
/// need (WP4.S2) and Grid never reads.
pub fn col_widths(rows: &[Vec<RenderedCell>], n_cols: usize) -> (Vec<usize>, Vec<usize>) {
    let mut widths = vec![0usize; n_cols];
    let mut min_widths = vec![0usize; n_cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell_display_width(cell));
            }
            if let Some(mw) = min_widths.get_mut(i) {
                *mw = (*mw).max(longest_atomic_unit_width(&cell.text));
            }
        }
    }
    (widths, min_widths)
}

/// The longest whitespace-delimited word in `text`, by display width — a
/// column's floor width for Wrapped layout (a `http://`/`https://`-prefixed
/// word is atomic by policy, `wrap_cell`'s own `is_url`, but its WIDTH is
/// measured the same as any other word; atomicity only changes whether
/// `wrap_cell` may split it, never how wide it is).
fn longest_atomic_unit_width(text: &str) -> usize {
    text.split_whitespace()
        .map(display_width)
        .max()
        .unwrap_or(0)
}

/// One flat visible char plus its provenance — the intermediate shape
/// [`grid_row`]/[`separator_row`] (and, in sibling modules, `wrapped::wrapped_row`/
/// `pivot::pivot_rows`) build before grouping into the
/// `(String, Vec<CellSrc>, ScopeId)` runs `row_spans` consumes. `pub(super)`:
/// visible crate-tree-wide under `table::`, so the Wrapped/Pivoted builders
/// (separate files, CONSTITUTION §1.6) share this exact shape instead of a
/// second copy of it.
pub(super) struct FlatChar {
    pub(super) ch: char,
    pub(super) buf: i64,
    pub(super) scope: ScopeId,
}

/// Groups a flat char sequence into maximal contiguous same-scope runs —
/// the shared tail every layout's row builder uses, so each builds its row
/// as one flat pass and lets run boundaries fall out of the scope changes
/// rather than being hand-tracked per column.
pub(super) fn group_runs(flat: &[FlatChar]) -> Vec<(String, Vec<CellSrc>, ScopeId)> {
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
    let content_w = cell_display_width(cell);
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

/// The three layouts a rendered table may choose, keyed off available
/// width (WP4.S1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableLayout {
    Grid,
    Wrapped,
    Pivoted,
}

/// Selects a table's layout for `avail` display columns (WP4.S1), given
/// each column's natural width (`widths`) and its floor/atomic width
/// (`min_widths`, `col_widths`'s second return). `avail == 0` means "no
/// width has been set" and always selects Grid unconditionally (Go parity:
/// an editor that never got a resize message never wraps a table either).
///
/// One deliberate correction against the Go reference (Assumption A1): the
/// grid-fit test below uses the table's TRUE rendered row width
/// `Σw + 3n + 1`, not Go's own minimum-grid-width formula (`Σw + 4n − 1`,
/// which disagrees with the width Go itself actually renders except at
/// exactly 2 columns). Every OTHER threshold is kept verbatim, including
/// `frame_overhead` below (`4n − 1`, the SAME formula the grid-fit test just
/// rejected) — that inconsistency is Assumption A1's documented, accepted
/// cost: at `n != 2`, within `n − 2` columns of the threshold, this picks a
/// different layout than Go would at the same width. Not harmonized away —
/// silently "fixing" only one of the two formulas would make the pair
/// agree with each other instead of with the rendered width either of them
/// is supposed to describe.
pub fn choose(widths: &[usize], min_widths: &[usize], avail: usize) -> TableLayout {
    let n = widths.len();
    if n == 0 || avail == 0 {
        return TableLayout::Grid;
    }

    let sum_w: usize = widths.iter().sum();
    let grid_total = sum_w + 3 * n + 1;
    if grid_total <= avail {
        return TableLayout::Grid;
    }

    let frame_overhead = 4 * n - 1;
    let content_budget = (avail as isize) - (frame_overhead as isize);
    if content_budget <= 0 {
        return TableLayout::Pivoted;
    }
    let content_budget = content_budget as usize;

    let min_flex = 12usize;
    let equal_share = content_budget / n;
    let mut atomic_budget = 0usize;
    let mut flex_count = 0usize;
    for i in 0..n {
        let min_w = min_widths.get(i).copied().unwrap_or(0);
        if min_w > equal_share {
            atomic_budget += min_w;
        } else {
            flex_count += 1;
        }
    }

    let flex_budget = content_budget.saturating_sub(atomic_budget);
    // Two distinct viability checks (Go parity, kept separate rather than
    // merged into one condition so each keeps its own name/shape): flexible
    // columns get enough room each, OR there are no flexible columns at all
    // and the atomic ones already fit.
    let flex_cols_have_room = flex_count > 0 && flex_budget >= flex_count * min_flex;
    let only_atomic_cols_fit = flex_count == 0 && atomic_budget <= content_budget;
    if flex_cols_have_room || only_atomic_cols_fit {
        TableLayout::Wrapped
    } else {
        TableLayout::Pivoted
    }
}

/// Shrinks `widths` to fit `content_budget` (Wrapped layout only, WP4.S3):
/// floor each column at `max(3, min_widths[i]).min(widths[i])`, then
/// distribute the remaining budget proportionally to each column's own
/// "stretch" demand (how much more it wants beyond its floor, never
/// exceeding its natural width), giving any rounding remainder to the
/// single widest column.
pub fn constrain_widths(
    widths: &[usize],
    min_widths: &[usize],
    content_budget: usize,
) -> Vec<usize> {
    let n = widths.len();
    if n == 0 {
        return Vec::new();
    }

    let mut result = vec![0usize; n];
    let mut floor_total = 0usize;
    for i in 0..n {
        let natural = widths.get(i).copied().unwrap_or(0);
        let min_w = min_widths.get(i).copied().unwrap_or(0);
        let floor = 3usize.max(min_w).min(natural);
        if let Some(r) = result.get_mut(i) {
            *r = floor;
        }
        floor_total += floor;
    }

    let remaining = (content_budget as isize) - (floor_total as isize);
    if remaining <= 0 {
        return result;
    }
    let remaining = remaining as usize;

    let total_stretch: usize = (0..n)
        .map(|i| {
            widths
                .get(i)
                .copied()
                .unwrap_or(0)
                .saturating_sub(result.get(i).copied().unwrap_or(0))
        })
        .sum();

    if total_stretch == 0 {
        let per_col = remaining / n;
        for r in &mut result {
            *r += per_col;
        }
        return result;
    }

    let mut leftover = remaining;
    for i in 0..n {
        let natural = widths.get(i).copied().unwrap_or(0);
        let stretch = natural.saturating_sub(result.get(i).copied().unwrap_or(0));
        if stretch == 0 {
            continue;
        }
        let alloc = ((stretch * remaining) / total_stretch).min(stretch);
        if let Some(r) = result.get_mut(i) {
            *r += alloc;
        }
        leftover = leftover.saturating_sub(alloc);
    }

    if leftover > 0 {
        let mut widest = 0usize;
        for i in 1..n {
            if widths.get(i).copied().unwrap_or(0) > widths.get(widest).copied().unwrap_or(0) {
                widest = i;
            }
        }
        if let Some(r) = result.get_mut(widest) {
            *r += leftover;
        }
    }

    result
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

    /// A grapheme cluster's CELL column, not its char index — alignment is
    /// a display-width property (a CJK cell's char count legitimately
    /// differs from its cell width, WP9.S3/[rune-md 6]), so both the total
    /// width comparison and the bar/corner position comparison below walk
    /// `graphemes(true)` and accumulate `grapheme_width`, never `.chars()`.
    fn cell_cols_of(text: &str, matches: impl Fn(&str) -> bool) -> Vec<usize> {
        let mut col = 0usize;
        let mut out = Vec::new();
        for g in text.graphemes(true) {
            if matches(g) {
                out.push(col);
            }
            col += grapheme_width(g);
        }
        out
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
        assert_eq!(display_width(sep_text), display_width(&grid_text));
        // Bars/corners must land at the SAME visual COLUMN (display cells)
        // in both rows, not the same char index.
        let bar_cols = cell_cols_of(&grid_text, |g| g == "│");
        let corner_cols = cell_cols_of(sep_text, |g| matches!(g, "├" | "┼" | "┤"));
        assert_eq!(bar_cols, corner_cols);
    }
}
