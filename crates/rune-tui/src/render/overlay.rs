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
/// 1. Derives the visible byte window `lo..hi` via [`visible_byte_range`].
///    Returns early with `rows` untouched if no cell is real.
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
///    window and painted `Some`, through `Theme::overlay_scope_style`
///    rather than `Theme::scope_style` — that variant always strips `bg`,
///    so a fence's own background survives underneath a token's foreground
///    colour regardless of whether the overlaid scope would otherwise
///    carry one.
///
/// Every index into `window` goes through `.get`/`.get_mut` (`indexing_
/// slicing` is a hard `deny` under `make lint`), never `[]`.
pub(super) fn apply_highlight_spans(
    rows: &mut [Vec<Cell>],
    spans: &[(Range<usize>, ScopeId)],
    theme: &Theme,
) {
    let Some(Range { start: lo, end: hi }) = visible_byte_range(rows) else {
        return;
    };

    let mut window: Vec<Option<ScopeId>> = vec![None; hi - lo];
    // `spans` is painter-order (`range.start` ASC, per the doc comment
    // above), so a span whose `start` already sits at or past `hi` — and
    // every span after it, since `start` only grows — cannot overlap the
    // visible window at all (plan WP16.S4). `partition_point` finds that
    // boundary in O(log n); the loop below then walks only the PREFIX up to
    // it, never the full span list regardless of where in a large parsed
    // document the current viewport happens to sit (`spans` is capped at
    // `rune_ts::MAX_SPANS` but a full parse of a large file can still fill
    // most of that cap while the viewport shows a small fraction of it).
    let visible_end = spans.partition_point(|(range, _)| range.start < hi);
    let visible = spans.get(..visible_end).unwrap_or(&[]);
    for (range, scope) in visible {
        if range.start >= range.end || range.end <= lo {
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
                cell.style = cell.style.patch(theme.overlay_scope_style(*id));
            }
        }
    }
}

/// Scans `rows` for the visible byte window `lo..hi` (the min/max
/// non-negative `buf_offset` seen, `hi` one past the max) — a decorative
/// cell (`buf_offset < 0`, none produced yet, see `Cell`'s docs) is skipped.
/// `None` when no cell in `rows` is real (an empty document, or every cell
/// decorative). Split out of `apply_highlight_spans` (the syntax-
/// highlighting-latency plan's WP3, D6) so `render::build_rows` can reuse
/// the identical window derivation to scope a per-frame `rune_ts::
/// highlight_range` query to the same bytes the span overlay itself paints
/// — one window, one definition, never re-derived.
pub(super) fn visible_byte_range(rows: &[Vec<Cell>]) -> Option<Range<usize>> {
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
    let lo = lo?;
    if hi <= lo { None } else { Some(lo..hi) }
}

/// Paints the caret and, per-cursor, its selection background — gated on
/// `show_overlays` (`Document::shows_caret`, folding in both focus and
/// read-only). Early-returning before the cursor loop, rather than a
/// caller-side `if`, covers both overlay kinds in one place (the selection
/// highlight is painted from inside this same loop) and means no future
/// caller can paint either without deciding whether this document may show
/// them — matching Go's `applyOverlays` gate (`textedit/render.go`), which
/// covers cursors and selections together.
pub(super) fn apply_cursor_overlays(
    show_overlays: bool,
    rows: &mut [Vec<Cell>],
    view: &ViewSnapshots,
    cursors: &CursorSet,
    buf: &Buffer,
    scroll_row: usize,
    theme: &Theme,
) {
    if !show_overlays {
        return;
    }
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
        // A boxed (Grid/Wrapped) table row's rendered width is a hard
        // invariant (`TABLE-ROW-WIDTH`): every content/border row in the
        // same group carries the SAME summed cell width, always — never
        // conditionally. `visual_col` is derived from `buffer_to_syntax`'s
        // generic hidden-range delta scheme, which only ever accounts for
        // CONCEALED inline markup (`emit`'s `hidden` bookkeeping); a table
        // row's raw source line is instead wholly SUBSTITUTED for a
        // rendered box, and a ragged row (more `|`-delimited cells than the
        // table's own column count, silently dropped by comrak's table
        // parser rather than rejected) can carry raw bytes far past the
        // substituted text's own length. Nothing narrows `visual_col` back
        // down to the row's real width in that case, so it can land AT or
        // PAST `row.len()` even though every visible cell has already been
        // walked — exactly the case `place_caret`'s below-EOL fallback
        // exists for on an ordinary (unboxed) line, where appending one
        // decorative cell is harmless. On a BOXED row it is never harmless:
        // it grows that one row a cell wider than every other row in its
        // group, tripping the invariant. So a boxed row never takes that
        // fallback at all — the caret clamps onto the row's own last cell
        // instead, the same way a caret can never be visually "past" a
        // closed box's own right border.
        let boxed = view
            .wrap
            .segments()
            .get(wrap_point.row)
            .and_then(|seg| seg.table.as_ref())
            .is_some_and(|t| t.boxed);
        place_caret(row, visual_col, cursor.position, boxed);
    }
}

fn highlight_selection(rows: &mut [Vec<Cell>], start: usize, end: usize, theme: &Theme) {
    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            if cell.buf_offset >= 0 {
                let offset = cell.buf_offset as usize;
                if offset >= start && offset < end {
                    // Go `Selection` (`styles.go`, WP2.S2 migration).
                    cell.style = cell.style.bg(theme.chrome.selection_bg);
                }
            }
        }
    }
}

/// Reverse-video the cell at `visual_col`, or — if the caret sits past the
/// last visible char on this row — append a synthetic EOL cursor cell (port
/// of Go `render.go`). Only ever reached when the caller has already
/// decided overlays are shown (`apply_cursor_overlays`'s `show_overlays`
/// gate) — none of its three REVERSED paths (this cell, the boxed-row last
/// cell below, or the synthetic EOL push) runs on an unfocused or
/// read-only document. `boxed` rows (a Grid/Wrapped table's own content and
/// border rows) never take that append branch: appending would make this
/// ONE row a cell wider than every other row in its table group, violating
/// `TABLE-ROW-WIDTH` (every row in a boxed group shares one summed width,
/// unconditionally). Reversing the row's own last cell instead keeps the
/// row's width exactly what every sibling row's width already is, by
/// construction — the caret still lands visibly inside the box, just
/// clamped to its last real column instead of stepping past the closing
/// border.
fn place_caret(row: &mut Vec<Cell>, visual_col: usize, buf_offset: usize, boxed: bool) {
    let mut col = 0usize;
    for cell in row.iter_mut() {
        if col == visual_col {
            cell.style = cell.style.add_modifier(RtModifier::REVERSED);
            return;
        }
        col += cell.width.max(1) as usize;
    }
    if boxed {
        if let Some(last) = row.last_mut() {
            last.style = last.style.add_modifier(RtModifier::REVERSED);
        }
        return;
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

    /// Plan WP16.S4: a span whose `start` sits past the visible window
    /// (`hi`) must still be excluded now that the window scan cuts off at
    /// `partition_point(start < hi)` instead of scanning every span — this
    /// pins that the cut doesn't accidentally paint (or panic on) a span
    /// that used to just be skipped by the old `range.start >= hi` filter.
    #[test]
    fn a_span_starting_past_the_visible_window_is_excluded() {
        let theme = Theme::catppuccin_mocha(false);
        let plain = Style::default();
        let mut rows = vec![(0..5).map(cell).collect::<Vec<Cell>>()];
        // Sorted by `start` ASC (painter order): one span inside the
        // window, one entirely past it.
        let spans = vec![(1..3, scope("variable")), (100..200, scope("function"))];
        apply_highlight_spans(&mut rows, &spans, &theme);

        let row = &rows[0];
        assert_eq!(row[0].style, plain);
        assert_eq!(row[1].style, theme.scope_style(scope("variable")));
        assert_eq!(row[2].style, theme.scope_style(scope("variable")));
        for i in [3, 4] {
            assert_eq!(row[i].style, plain, "cell {i} is outside every span");
        }
    }

    /// A span that starts before the window's `hi` but extends past `lo`'s
    /// window into the document tail must still paint its portion INSIDE
    /// the window — the `partition_point` cut is on `start`, not `end`, so
    /// a wide span isn't dropped just because it outlives the window.
    #[test]
    fn a_span_straddling_the_window_boundary_still_paints_its_visible_portion() {
        let theme = Theme::catppuccin_mocha(false);
        let mut rows = vec![(0..5).map(cell).collect::<Vec<Cell>>()];
        let spans = vec![(2..1000, scope("function"))];
        apply_highlight_spans(&mut rows, &spans, &theme);

        let row = &rows[0];
        assert_eq!(row[0].style, Style::default());
        assert_eq!(row[1].style, Style::default());
        for i in [2, 3, 4] {
            assert_eq!(row[i].style, theme.scope_style(scope("function")));
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

    /// The `TABLE-ROW-WIDTH` regression `place_caret`'s `boxed` branch
    /// exists for (`crates/rune-fuzz/proptest-regressions/human_session.txt`,
    /// seed `cc 5f23e392...`), exercised directly rather than through the
    /// full `App`/`DocMachine` pipeline: the caret gate this file's
    /// `apply_cursor_overlays` now applies (`show_overlays`, this ticket)
    /// and a table's `RevealGrant::ForceRendered`/`Decide` split
    /// (`rune_md::element::doc::DocMachine`) key off the exact same
    /// `Document::focused` bit — a table containing the cursor is only ever
    /// BOXED while unfocused, and the caret gate now suppresses painting
    /// entirely while unfocused, so a full-pipeline test can no longer
    /// reach this branch with a caret actually on screen. The clamp logic
    /// itself is still real (a non-markdown pathway, or a future Decide
    /// policy change, could still reach a boxed row with the caret
    /// visible), so it keeps its own direct coverage here instead.
    #[test]
    fn place_caret_clamps_onto_a_boxed_rows_last_cell_instead_of_appending() {
        let mut row: Vec<Cell> = (0..3).map(cell).collect();
        let before_len = row.len();
        // Far past the row's own rendered width (3 cells) — the ragged-row
        // case's dropped trailing `|`-cells produce exactly this: a
        // `visual_col` past every real cell on a boxed row.
        place_caret(&mut row, 100, 0, true);
        assert_eq!(
            row.len(),
            before_len,
            "a boxed row must never grow a cell wider from the caret clamp"
        );
        assert!(
            row.last()
                .is_some_and(|c| c.style.add_modifier.contains(RtModifier::REVERSED)),
            "the clamp must still reverse-video the row's own last cell"
        );
    }

    /// The unboxed counterpart: an ordinary (non-table) row past its last
    /// visible char DOES grow by one synthetic EOL cursor cell — the
    /// `TABLE-ROW-WIDTH` exemption is boxed-rows-only.
    #[test]
    fn place_caret_appends_a_synthetic_eol_cell_on_an_unboxed_row() {
        let mut row: Vec<Cell> = (0..3).map(cell).collect();
        place_caret(&mut row, 100, 7, false);
        assert_eq!(
            row.len(),
            4,
            "an unboxed row past its last cell must grow by one"
        );
        assert!(
            row.last()
                .is_some_and(|c| c.style.add_modifier.contains(RtModifier::REVERSED)),
            "the appended synthetic cell must carry the caret's reverse-video"
        );
    }
}
