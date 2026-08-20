//! Cursor/selection overlays, split out of `render` (500-line budget):
//! `build_rows` calls `apply_highlight_spans` after collecting a row's plain
//! `segment_cells` and painting the code-region background rectangle
//! underneath them, then `apply_cursor_overlays`, patching in the selection
//! background and the caret's reverse-video AFTER the token colours — so a
//! selection or the caret always wins over a highlight, exactly as it did
//! when the cursor overlays alone lived in `render` itself.

use std::ops::Range;

use ratatui::style::{Modifier as RtModifier, Style};

use rune_core::assert_invariant;
use rune_core::buffer::Buffer;
use rune_core::coords::{DisplayRow, WrapRow};
use rune_core::cursor::CursorSet;
use rune_md::element::doc::ViewSnapshots;
use rune_syntax::ScopeId;

use crate::theme::Theme;

use super::Cell;

/// Paints `spans` (outer-first painter, never a
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
///    cost the highlight budget/window design exists to avoid.
/// 3. Writes each span's `ScopeId` across its overlap with the window,
///    outer/earlier spans first — a later `.get_mut` write simply
///    overwrites an earlier one at the same byte, which is the painter
///    resolution rule.
/// 4. Walks `rows` once more, patching (`Style::patch`, never plain
///    assignment) each real cell whose byte fell in the
///    window and painted `Some`, through `Theme::overlay_scope_style`
///    rather than `Theme::scope_style` — that variant always strips `bg`,
///    so the code-region background rectangle painted before this pass
///    survives underneath a token's foreground colour regardless of
///    whether the overlaid scope would otherwise carry one.
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
    // visible window at all. `partition_point` finds that
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
            let Some(offset) = cell.buf_offset else {
                continue;
            };
            let offset = offset as usize;
            if offset < lo || offset >= hi {
                continue;
            }
            if let Some(Some(id)) = window.get(offset - lo) {
                cell.style = cell.style.patch(theme.overlay_scope_style(*id));
            }
        }
    }
}

/// Scans `rows` for the visible byte window `lo..hi` (the min/max real
/// `buf_offset` seen, `hi` one past the max) — a decorative cell
/// (`buf_offset: None`, carries no buffer position at all, see `Cell`'s
/// docs) is skipped.
/// `None` when no cell in `rows` is real (an empty document, or every cell
/// decorative). Split out of `apply_highlight_spans` so `render::build_rows`
/// can reuse
/// the identical window derivation to scope a per-frame `rune_ts::
/// highlight_range` query to the same bytes the span overlay itself paints
/// — one window, one definition, never re-derived.
pub(super) fn visible_byte_range(rows: &[Vec<Cell>]) -> Option<Range<usize>> {
    let mut lo: Option<usize> = None;
    let mut hi: usize = 0;
    for row in rows.iter() {
        for cell in row.iter() {
            let Some(offset) = cell.buf_offset else {
                continue;
            };
            let offset = offset as usize;
            lo = Some(lo.map_or(offset, |current| current.min(offset)));
            hi = hi.max(offset + 1);
        }
    }
    let lo = lo?;
    if hi <= lo { None } else { Some(lo..hi) }
}

/// The two overlay gates [`apply_cursor_overlays`] paints under — grouped
/// into one value rather than two loose `bool` parameters purely to keep
/// that function's own argument count under the repo's `too_many_arguments`
/// deny (the repo bans silencing it with an `#[allow]` instead).
#[derive(Clone, Copy)]
pub(crate) struct OverlayGates {
    /// Whether the caret may be painted — `Document::has_insertion_point`.
    pub caret: bool,
    /// Whether the selection background may be painted —
    /// `Document::shows_selection`.
    pub selection: bool,
}

/// Paints the caret and, per-cursor, its selection background — under two
/// SEPARATE gates in `gates`, rather than the one combined gate this
/// function used to take. A read-only document has no insertion point for a
/// caret to mark — there is nowhere for keystrokes to land — but a mouse
/// selection in it is real: the user can still `⌘C` it, and a selection that
/// copies without ever painting is a user action with no visible feedback.
/// So a read-only document may show `selection` while `caret` stays false;
/// an unfocused document, by contrast, must show neither, since nothing in
/// it should look interactive at all. Both gates still route through this
/// one function rather than two separate ones — no future caller can paint
/// either overlay without deciding, for each, whether this document may
/// show it — and the early return below covers the case where neither may
/// be painted at all.
pub(crate) fn apply_cursor_overlays(
    gates: OverlayGates,
    rows: &mut [Vec<Cell>],
    view: &ViewSnapshots,
    cursors: &CursorSet,
    buf: &Buffer,
    scroll_row: DisplayRow,
    theme: &Theme,
) {
    let OverlayGates { caret, selection } = gates;
    if !caret && !selection {
        return;
    }
    for cursor in cursors.all() {
        if selection && cursor.has_selection() {
            let (start, end) = cursor.selection_range();
            highlight_selection(rows, start, end, theme);
        }

        if !caret {
            continue;
        }

        let buffer_point = buf.offset_to_line_col(cursor.position);
        let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
        // The cursor's own row lives in WRAP space (border rows aren't
        // addressable by the caret); convert to the DISPLAY row `rows` is
        // now indexed by before comparing against/indexing off `scroll_row`
        // (also display-space).
        let display_row = view.display.wrap_to_display(WrapRow(wrap_point.row));
        if display_row < scroll_row {
            continue;
        }
        let Some(row) = rows.get_mut(display_row.0 - scroll_row.0) else {
            continue;
        };
        // `place_caret` walks cells POSITIONALLY and adds
        // `REVERSED` to whichever one sits at `visual_col`, with no
        // `buf_offset` check — `REVERSED` swaps fg and bg, which would
        // destroy a live placeholder cell's smuggled 24-bit image id
        // (`render::image::row_cells`'s own doc comment: `style.fg` IS the
        // id). Moot for a read-only image DOCUMENT (`has_insertion_point` is
        // `focused && !is_read_only()`, and an image document is always
        // read-only), but very much live for an inline embed inside an
        // otherwise-editable markdown document — an anchor row's own
        // `ImageRowRef` (whole-document OR embed, `target` either way)
        // means this row's cells may be exactly that kind of decorative
        // placeholder, so the caret is suppressed on it entirely instead of
        // painted, the same "this row's cells are not what they look like"
        // precedent `place_caret`'s own `boxed` parameter already
        // establishes for a table's border row.
        let on_image_row = view
            .display
            .rows()
            .get(display_row.0)
            .is_some_and(|r| r.image.is_some());
        if on_image_row {
            continue;
        }
        // `visual_col` above is wrap-space, unaware of the row's
        // own decoration prefix (`build_rows` prepends it before this
        // function runs). Shifting it right by that prefix's width is what
        // keeps `place_caret`'s cell walk landing on the SAME visible
        // column the decor-shifted row actually put the caret's target
        // char at — a decorated line CAN carry
        // the caret itself (an unfocused pane's forced conceal, or a
        // wrapped list item's continuation row keeping the first row's own
        // bullet), so this is load-bearing, not a cosmetic nicety.
        let visual_col = view
            .wrap
            .visual_col(buf.content(), wrap_point.row, wrap_point.col)
            + view
                .display
                .rows()
                .get(display_row.0)
                .map_or(0, super::decor::decor_cell_width) as usize;
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
            if let Some(offset) = cell.buf_offset {
                let offset = offset as usize;
                if offset >= start && offset < end {
                    cell.style = cell.style.bg(theme.chrome.selection_bg);
                }
            }
        }
    }
}

/// Reverse-video every cell tied at `visual_col`, or — if the caret sits
/// past the last visible char on this row — append a synthetic EOL cursor
/// cell. Only ever reached when the caller has already decided the caret
/// may be shown (`apply_cursor_overlays`'s `caret` gate) — none of its three
/// REVERSED paths (the tied cells, the boxed-row last cell below, or the
/// synthetic EOL push) runs on an unfocused or read-only document. `boxed`
/// rows (a Grid/Wrapped table's own content and border rows) never take
/// that append branch: appending would make this ONE row a cell wider than
/// every other row in its table group, violating `TABLE-ROW-WIDTH` (every
/// row in a boxed group shares one summed width, unconditionally).
/// Reversing the row's own last cell instead keeps the row's width exactly
/// what every sibling row's width already is, by construction — the caret
/// still lands visibly inside the box, just clamped to its last real column
/// instead of stepping past the closing border.
///
/// "Every cell tied at `visual_col`", not just the first: a width-0 `Cell`
/// (a lone zero-width rune, `grapheme_width`'s doc) starts at the SAME
/// column as whatever `Cell` follows it, since it advances the column
/// counter by nothing — `blit` then overwrites that zero-width `Cell`'s own
/// buffer position with the following `Cell`'s glyph, so only the LAST cell
/// in a tied run is what a reader actually sees. Reversing every cell in
/// the run, not only the first one `col == visual_col` finds, keeps
/// `CUR-CELL-SYNC` satisfied for whichever of them a cursor's own
/// `buf_offset` claims AND keeps the caret visible on the glyph `blit`
/// actually paints there, whichever cell that turns out to be.
fn place_caret(row: &mut Vec<Cell>, visual_col: usize, buf_offset: usize, boxed: bool) {
    let mut col = 0usize;
    let mut matched = false;
    for cell in row.iter_mut() {
        if col == visual_col {
            cell.style = cell.style.add_modifier(RtModifier::REVERSED);
            matched = true;
        } else if matched || col > visual_col {
            break;
        }
        col += cell.width as usize;
    }
    if matched {
        return;
    }
    if boxed {
        if let Some(last) = row.last_mut() {
            last.style = last.style.add_modifier(RtModifier::REVERSED);
        }
        return;
    }
    let synthetic_offset = u32::try_from(buf_offset).ok();
    assert_invariant!(synthetic_offset.is_some(), || format!(
        "caret byte offset {buf_offset} exceeds the cell offset range"
    ));
    row.push(Cell {
        text: " ".into(),
        width: 1,
        style: Style::default().add_modifier(RtModifier::REVERSED),
        buf_offset: synthetic_offset,
    });
}

/// Unit tests for [`apply_highlight_spans`] — hand-built
/// `(rows, spans)` pairs, not a real document. `apply_highlight_spans` is
/// `pub(super)` (this file's own encapsulation convention: every other
/// overlay function here is `pub(super)` too, reached only through
/// `render::build_rows`), so it is unreachable from the crate's external
/// `tests/` integration tests — those exercise the SAME algorithm
/// end-to-end instead, through a document's stored region highlight state
/// (`HighlightState::regions`, queried by `highlight::visible_spans`) and
/// `render::build_rows`/`testgrid`. This module covers the painter
/// resolution itself directly.

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "overlay_tests.rs"]
mod tests;
