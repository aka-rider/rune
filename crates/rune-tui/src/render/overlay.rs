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

// Paints `spans` onto `rows`' `Cell::style`, keyed on `Cell::buf_offset`.
// `spans` must already be in painter order (`start` ASC, `end` DESC,
// capture-yield-order ASC — `rune_ts::highlight` guarantees this) so an
// enclosing capture paints before anything nested inside it, and a later
// span at the same byte always overwrites an earlier one — reproducing
// tree-sitter-highlight's innermost-and-last-wins resolution. Patches via
// `Theme::overlay_scope_style`, which always strips `bg`, so a code
// region's background rectangle survives underneath a token's colour.
pub(super) fn apply_highlight_spans(
    rows: &mut [Vec<Cell>],
    spans: &[(Range<usize>, ScopeId)],
    theme: &Theme,
) {
    let Some(Range { start: lo, end: hi }) = visible_byte_range(rows) else {
        return;
    };

    let mut window: Vec<Option<ScopeId>> = vec![None; hi - lo];
    // `spans` is painter-order by `start`, so once a span's `start` is at
    // or past `hi` neither it nor anything after it can overlap the
    // visible window — `partition_point` finds that cut in O(log n) so a
    // large parse never gets walked fully for a small viewport.
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

// The visible byte window `lo..hi` covered by `rows`' real cells (`hi`
// one past the max) — `None` when every cell is decorative.
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

#[derive(Clone, Copy)]
pub(crate) struct OverlayGates {
    pub caret: bool,
    pub selection: bool,
}

// Paints the caret and, per-cursor, its selection background — under two
// SEPARATE gates in `gates` rather than one combined gate: a read-only
// document has no insertion point for a caret to mark, but a mouse
// selection in it is real (the user can still copy it), so it may show
// `selection` while `caret` stays false. An unfocused document shows
// neither.
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
            highlight_selection(rows, start.get(), end.get(), theme);
        }

        if !caret {
            continue;
        }

        let buffer_point = buf.offset_to_line_col(cursor.position.get());
        let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
        let display_row = view.display.wrap_to_display(WrapRow(wrap_point.row));
        if display_row < scroll_row {
            continue;
        }
        let Some(row) = rows.get_mut(display_row.0 - scroll_row.0) else {
            continue;
        };
        let on_image_row = view
            .display
            .rows()
            .get(display_row.0)
            .is_some_and(|r| r.image.is_some());
        if on_image_row {
            continue;
        }
        let visual_col = view
            .wrap
            .visual_col(buf.content(), wrap_point.row, wrap_point.col)
            + view
                .display
                .rows()
                .get(display_row.0)
                .map_or(0, super::decor::decor_cell_width) as usize;
        let boxed = view
            .wrap
            .segments()
            .get(wrap_point.row)
            .and_then(|seg| seg.table.as_ref())
            .is_some_and(|t| t.boxed);
        place_caret(row, visual_col, cursor.position.get(), boxed);
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

// Reverse-video every cell tied at `visual_col`, not just the first: a
// width-0 `Cell` (a lone zero-width rune) starts at the same column as
// whatever `Cell` follows it, and `blit` overwrites its buffer position
// with the following cell's glyph — so only the LAST cell in a tied run
// is what a reader actually sees, and every one of them must be reversed
// to keep the caret visible on whichever glyph ends up painted there.
//
// A `boxed` row (a Grid/Wrapped table's own content/border row) never
// appends the synthetic EOL cell a caret past the last visible char would
// otherwise get — every row in a boxed table group shares one summed
// width, so it reverses the row's own last cell instead.
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
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "overlay_tests.rs"]
mod tests;
