//! Cursor/selection overlays, split out of `render` (§1.6 budget):
//! `build_rows` calls `apply_highlight_spans` after collecting a row's plain
//! `segment_cells`, then `apply_cursor_overlays`, patching in the selection
//! background and the caret's reverse-video AFTER the token colours — so a
//! selection or the caret always wins over a highlight, exactly as it did
//! when the cursor overlays alone lived in `render` itself.

use std::ops::Range;

use ratatui::style::{Modifier as RtModifier, Style};

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::ViewSnapshots;
use rune_syntax::ScopeId;

use crate::theme::Theme;

use super::Cell;

/// Paints `spans` (plan WP5.S5, decision 3: outer-first painter, never a
/// per-cell search) onto `rows`' `Cell::style`, keyed on `Cell::buf_offset`
/// exactly as `highlight_selection` below keys on it for the selection
/// background. `spans` must already be in painter order — `(start ASC, end
/// DESC, capture-yield-order ASC)`, which `rune_ts::highlight` guarantees —
/// so an enclosing capture is always painted before anything nested inside
/// it and a later query pattern on the same node always overwrites an
/// earlier one, reproducing `tree-sitter-highlight`'s own innermost-and-
/// last-wins resolution.
///
/// 1. Scans `rows` for the visible byte window `lo..hi` (the min/max
///    non-negative `buf_offset` seen, `hi` one past the max) — a decorative
///    cell (`buf_offset < 0`, none produced yet, see `Cell`'s docs) is
///    skipped. Returns early with `rows` untouched if no cell is real.
/// 2. Allocates one scratch slot per VISIBLE byte, never per document byte —
///    `apply_cursor_overlays` (`build_rows`'s very next call) already
///    bounds render's own cost to the visible viewport the same way; a
///    per-document allocation here would silently reintroduce the O(len)
///    cost the WP5 budget/window design exists to avoid.
/// 3. Writes each span's `ScopeId` across its overlap with the window,
///    outer/earlier spans first — a later `.get_mut` write simply
///    overwrites an earlier one at the same byte, which is the painter
///    resolution rule.
/// 4. Walks `rows` once more, patching (`Style::patch`, never plain
///    assignment — decision 2) each real cell whose byte fell in the
///    window and painted `Some`. `patch` only sets `fg`/modifiers
///    (`code_scope_style` never sets a `bg`), so a fence's `markup.raw.
///    block` background survives underneath a token's foreground colour.
///
/// Every index into `window` goes through `.get`/`.get_mut` (`indexing_
/// slicing` is a hard `deny` under `make lint`), never `[]`.
pub(super) fn apply_highlight_spans(
    rows: &mut [Vec<Cell>],
    spans: &[(Range<usize>, ScopeId)],
    theme: &Theme,
) {
    let mut lo: Option<usize> = None;
    let mut hi: usize = 0;
    for row in rows.iter() {
        for cell in row.iter() {
            if cell.buf_offset < 0 {
                continue;
            }
            let offset = cell.buf_offset as usize;
            lo = Some(lo.map_or(offset, |current| current.min(offset)));
            hi = hi.max(offset + 1);
        }
    }
    let Some(lo) = lo else { return };
    if hi <= lo {
        return;
    }

    let mut window: Vec<Option<ScopeId>> = vec![None; hi - lo];
    for (range, scope) in spans {
        if range.start >= range.end || range.end <= lo || range.start >= hi {
            continue;
        }
        let start = range.start.max(lo) - lo;
        let end = range.end.min(hi) - lo;
        if let Some(slots) = window.get_mut(start..end) {
            for slot in slots.iter_mut() {
                *slot = Some(*scope);
            }
        }
    }

    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            if cell.buf_offset < 0 {
                continue;
            }
            let offset = cell.buf_offset as usize;
            if offset < lo || offset >= hi {
                continue;
            }
            if let Some(Some(id)) = window.get(offset - lo) {
                cell.style = cell.style.patch(theme.scope_style(*id));
            }
        }
    }
}

pub(super) fn apply_cursor_overlays(
    rows: &mut [Vec<Cell>],
    view: &ViewSnapshots,
    cursors: &CursorSet,
    buf: &Buffer,
    scroll_row: usize,
    theme: &Theme,
) {
    for cursor in cursors.all() {
        if cursor.has_selection() {
            let (start, end) = cursor.selection_range();
            highlight_selection(rows, start, end, theme);
        }

        let buffer_point = buf.offset_to_line_col(cursor.position);
        let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
        // The cursor's own row lives in WRAP space (border rows aren't
        // addressable by the caret); convert to the DISPLAY row `rows` is
        // now indexed by before comparing against/indexing off `scroll_row`
        // (also display-space, WP3.S5).
        let display_row = view.display.wrap_to_display(wrap_point.row);
        if display_row < scroll_row {
            continue;
        }
        let Some(row) = rows.get_mut(display_row - scroll_row) else {
            continue;
        };
        let visual_col = view
            .wrap
            .visual_col(buf.content(), wrap_point.row, wrap_point.col);
        place_caret(row, visual_col, cursor.position);
    }
}

fn highlight_selection(rows: &mut [Vec<Cell>], start: usize, end: usize, theme: &Theme) {
    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            if cell.buf_offset >= 0 {
                let offset = cell.buf_offset as usize;
                if offset >= start && offset < end {
                    // Go `Selection` (`styles.go:196`, WP2.S2 migration).
                    cell.style = cell.style.bg(theme.chrome.selection_bg);
                }
            }
        }
    }
}

/// Reverse-video the cell at `visual_col`, or — if the caret sits past the
/// last visible char on this row — append a synthetic EOL cursor cell (port
/// of Go `render.go:151-176`).
fn place_caret(row: &mut Vec<Cell>, visual_col: usize, buf_offset: usize) {
    let mut col = 0usize;
    for cell in row.iter_mut() {
        if col == visual_col {
            cell.style = cell.style.add_modifier(RtModifier::REVERSED);
            return;
        }
        col += cell.width.max(1) as usize;
    }
    row.push(Cell {
        text: " ".to_string(),
        width: 1,
        style: Style::default().add_modifier(RtModifier::REVERSED),
        buf_offset: buf_offset as i64,
    });
}

/// Unit tests for [`apply_highlight_spans`] (plan WP5.S7) — hand-built
/// `(rows, spans)` pairs, not a real document. `apply_highlight_spans` is
/// `pub(super)` (this file's own encapsulation convention: every other
/// overlay function here is `pub(super)` too, reached only through
/// `render::build_rows`), so it is unreachable from the crate's external
/// `tests/` integration tests — those exercise the SAME algorithm
/// end-to-end instead, through `Document::highlight.spans` and
/// `render::build_rows`/`testgrid`. This module covers the painter
/// resolution itself directly.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use rune_syntax::scope::scope_table;

    fn cell(offset: i64) -> Cell {
        Cell {
            text: "x".to_string(),
            width: 1,
            style: Style::default(),
            buf_offset: offset,
        }
    }

    fn scope(name: &str) -> ScopeId {
        scope_table().resolve(name).expect("known scope name")
    }

    /// Decision 3: an outer span painted first, then a nested span painted
    /// over it, leaves the nested bytes with the INNER style and everything
    /// else with the OUTER one — innermost-wins, no per-cell search.
    #[test]
    fn nested_span_overwrites_the_outer_one_it_sits_inside() {
        let theme = Theme::catppuccin_mocha(false);
        let function_style = theme.scope_style(scope("function"));
        let variable_style = theme.scope_style(scope("variable"));

        let mut rows = vec![(0..10).map(cell).collect::<Vec<Cell>>()];
        let spans = vec![(0..10, scope("function")), (3..5, scope("variable"))];
        apply_highlight_spans(&mut rows, &spans, &theme);

        let row = &rows[0];
        for i in [0, 1, 2, 5, 6, 7, 8, 9] {
            assert_eq!(
                row[i].style, function_style,
                "cell {i} should be function-styled"
            );
        }
        for i in [3, 4] {
            assert_eq!(
                row[i].style, variable_style,
                "cell {i} should be variable-styled"
            );
        }
    }

    /// The overlay patches `style` only — every cell's `buf_offset`/`width`
    /// must come out byte-identical to what went in.
    #[test]
    fn overlay_changes_style_only_never_offset_or_width() {
        let theme = Theme::catppuccin_mocha(false);
        let before: Vec<Cell> = (0..10).map(cell).collect();
        let mut rows = vec![before.clone()];
        let spans = vec![(0..10, scope("function")), (3..5, scope("variable"))];
        apply_highlight_spans(&mut rows, &spans, &theme);

        for (b, a) in before.iter().zip(rows[0].iter()) {
            assert_eq!(b.buf_offset, a.buf_offset);
            assert_eq!(b.width, a.width);
        }
    }

    /// No visible (non-decorative) cell means nothing to paint — the window
    /// scan returns early and `rows` is left exactly as it was.
    #[test]
    fn all_decorative_cells_leave_rows_untouched() {
        let theme = Theme::catppuccin_mocha(false);
        let mut rows = vec![vec![cell(-1), cell(-1)]];
        let before = rows.clone();
        apply_highlight_spans(&mut rows, &[(0..2, scope("function"))], &theme);
        assert_eq!(rows, before);
    }
}
