//! Cell-model invariants: `CELL-OFFSET`/`CELL-NO-EOL`/`CELL-ORDER` over
//! `Painted.cells`, plus the pure comparator half of `SYNC-IDEMPOTENT`.
//!
//! `SYNC-IDEMPOTENT` itself needs a SECOND live `app.sync_view()` call with
//! no intervening message — it is a comparison of two consecutive renders
//! of the same settled state, not a property of one `Snapshot` — so
//! `driver.rs` drives the two `render::build_rows` calls directly against
//! `&mut App` (G6 proves this is a genuine fixpoint: `Document::view` never
//! reads `viewport.scroll_row`, and `Viewport::reconcile` converges in one
//! call, plan WP7.S1) and hands the results to `sync_idempotent` below,
//! which is the actual pure, hand-testable assertion.

use ratatui::buffer::CellWidth;
use rune_tui::render::Cell;

use super::Violation;
use crate::snapshot::Painted;

/// `SYNC-IDEMPOTENT`'s display-pipeline half — compares the production
/// (WP16-memoized) render already cached on `Document` against a
/// cache-BYPASSED rebuild from the exact same already-synced inputs
/// (`DocMachine::force_rebuild`, `driver/checks.rs`). Once `snapshot()`
/// memoizes on a `dirty` flag, comparing two ordinary `sync_view()` calls
/// would only ever compare a memo hit against itself — trivially equal
/// regardless of whether the underlying emit -> wrap -> display pass is
/// actually a fixpoint (CODE-REVIEW.md rune-fuzz finding 1). Forcing a
/// genuine second rebuild is what makes this check able to fail again.
/// Active-document-switch-safe: both row sets come from the SAME already-
/// synced active document (`driver/checks.rs::sync_idempotent_check`) with
/// no message, let alone a document switch, between them.
pub fn sync_idempotent_rebuild(
    production_rows: &[Vec<Cell>],
    rebuilt_rows: &[Vec<Cell>],
) -> Option<Violation> {
    if production_rows != rebuilt_rows {
        return Some(Violation::new(
            "SYNC-IDEMPOTENT",
            format!(
                "a cache-bypassed rebuild from the same synced inputs produced different rows \
                 than the memoized production render ({} rows production, {} rows rebuilt)",
                production_rows.len(),
                rebuilt_rows.len()
            ),
        ));
    }
    None
}

/// `SYNC-IDEMPOTENT`'s scroll half — `rows_before`/`scroll_before` are
/// captured immediately before a second, message-free `app.sync_view()`;
/// `rows_after`/`scroll_after` immediately after. `rows_before`/
/// `rows_after` are both ordinary (memoized) production renders here — a
/// genuine display-pipeline regression is caught by
/// [`sync_idempotent_rebuild`] above instead, since a memo hit would make
/// the row comparison here vacuous; this pair still catches a
/// non-settling `Viewport::reconcile` scroll, which memoization never
/// masks (G6).
/// Active-document-switch-safe: both halves are captured around a single,
/// message-free `app.sync_view()` call (`driver/checks.rs`) — nothing can
/// switch `app.active` in between.
pub fn sync_idempotent(
    rows_before: &[Vec<Cell>],
    scroll_before: usize,
    rows_after: &[Vec<Cell>],
    scroll_after: usize,
) -> Option<Violation> {
    if rows_before != rows_after {
        return Some(Violation::new(
            "SYNC-IDEMPOTENT",
            format!(
                "a second sync_view() with no intervening message changed the rendered rows \
                 ({} rows before, {} rows after)",
                rows_before.len(),
                rows_after.len()
            ),
        ));
    }
    if scroll_before != scroll_after {
        return Some(Violation::new(
            "SYNC-IDEMPOTENT",
            format!(
                "a second sync_view() with no intervening message moved scroll_row: \
                 {scroll_before} -> {scroll_after}"
            ),
        ));
    }
    None
}

/// `SCROLL-IN-DOC` (L0) — `Painted.scroll_row` is strictly less than
/// `Painted.total_rows`: the viewport never scrolls to or past a row the
/// document doesn't have. `total_rows` is always >= 1 (an empty buffer
/// still yields one row), so this also catches `scroll_row` left nonzero
/// against a one-row document. Closes the hole `CUR-BOUNDS`/`CUR-NO-CARET-
/// HIDDEN` deliberately leave open for a cursor legitimately scrolled
/// outside the viewport (`invariant/cursor.rs`): this checks the viewport
/// itself, not a cursor's relation to it, so no such carve-out applies —
/// `scroll_row` past the document's last row is never legitimate
/// (`Viewport::reconcile`'s clamp, `viewport.rs`).
///
/// Active-document-switch-safe: L0, one `Painted`'s own `scroll_row`
/// against its own `total_rows`.
pub fn scroll_in_doc(painted: &Painted) -> Option<Violation> {
    if painted.scroll_row.0 >= painted.total_rows {
        return Some(Violation::new(
            "SCROLL-IN-DOC",
            format!(
                "scroll_row={} is not strictly less than total_rows={}",
                painted.scroll_row.0, painted.total_rows
            ),
        ));
    }
    None
}

/// `CELL-OFFSET` (L0, sampled per G19) — every
/// `Cell.buf_offset` is `None` (decorative) or a valid, in-bounds,
/// char-boundary byte offset into `content`. A real offset's `width` may be
/// `0` ONLY when ratatui itself derives `0` for that same `Cell.text` (a
/// lone zero-width rune — a bare combining mark, a stray ZWJ, a lone
/// variation selector, a lone zero-width space — `rune_syntax::wrap::
/// grapheme_width`'s doc); any OTHER width-0 real cell is a producer bug —
/// a real buffer byte whose width silently disagrees with what a terminal
/// would draw for it. The old "negative but not the `-1` sentinel" arm is
/// gone: `Option<u32>` makes negative garbage unrepresentable by
/// construction, which is the point of the type.
///
/// Active-document-switch-safe: L0, checks one `Painted`'s `cells` against
/// its own `content`.
pub fn cell_offset(painted: &Painted) -> Option<Violation> {
    for row in &painted.cells {
        for cell in row {
            let Some(offset) = cell.buf_offset else {
                continue;
            };
            let offset = offset as usize;
            if offset > painted.content.len() || !painted.content.is_char_boundary(offset) {
                return Some(Violation::new(
                    "CELL-OFFSET",
                    format!(
                        "cell buf_offset={offset} is out of bounds or not a char boundary \
                         (content.len()={})",
                        painted.content.len()
                    ),
                ));
            }
            if cell.width == 0 && usize::from(cell.text.cell_width()) != 0 {
                return Some(Violation::new(
                    "CELL-OFFSET",
                    format!(
                        "cell at buf_offset={offset} has width=0 but ratatui derives \
                         {} for its own text {:?}",
                        cell.text.cell_width(),
                        cell.text
                    ),
                ));
            }
        }
    }
    None
}

/// `CELL-NO-EOL` (L0, sampled per G19) — no cell's `text` is `\n`
/// or `\r`: those bytes carry zero display width and must never reach a
/// rendered cell (`push_grapheme_cells` drops them, `render.rs`). A
/// grapheme cluster is never a bare `\n`/`\r` plus anything else (both
/// break grapheme segmentation), so an exact string comparison is safe.
///
/// Active-document-switch-safe: L0, single `Painted`.
pub fn cell_no_eol(painted: &Painted) -> Option<Violation> {
    for row in &painted.cells {
        for cell in row {
            if cell.text == "\n" || cell.text == "\r" {
                return Some(Violation::new(
                    "CELL-NO-EOL",
                    format!(
                        "cell carries an EOL char {:?} at buf_offset={:?}",
                        cell.text, cell.buf_offset
                    ),
                ));
            }
        }
    }
    None
}

/// `CELL-ORDER` (L0, sampled per G19) — within each row, cells
/// with a real (`Some`) `buf_offset` are non-decreasing left to right.
///
/// Active-document-switch-safe: L0, single `Painted`.
pub fn cell_order(painted: &Painted) -> Option<Violation> {
    for row in &painted.cells {
        let mut last: Option<u32> = None;
        for cell in row {
            let Some(offset) = cell.buf_offset else {
                continue;
            };
            if let Some(prev_offset) = last
                && offset < prev_offset
            {
                return Some(Violation::new(
                    "CELL-ORDER",
                    format!("row cell buf_offsets go backwards: {prev_offset} then {offset}"),
                ));
            }
            last = Some(offset);
        }
    }
    None
}

/// `TABLE-ROW-WIDTH` (L0, sampled per G19; plan WP5.S3) — within one
/// `table_group`, every row (a content row or a synthesised border) has
/// the same summed cell width. `Painted.cells`/`Painted.row_meta` are
/// index-aligned (`rune_tui::row_meta::row_meta` windows the exact same
/// `viewport.scroll_row`/`height` `render::build_rows` does), so pairing
/// `cells[i]` with `row_meta[i]` is always the SAME row. A border row
/// whose width disagrees with its content rows is exactly the defect
/// class this invariant exists to catch (plan WP5's own docs).
///
/// Active-document-switch-safe: L0, single `Painted`'s `cells`/`row_meta`.
pub fn table_row_width(painted: &Painted) -> Option<Violation> {
    let mut first_of_group: std::collections::HashMap<usize, (usize, usize)> =
        std::collections::HashMap::new();
    for (i, m) in painted.row_meta.iter().enumerate() {
        let Some(group) = m.table_group else {
            continue;
        };
        // Only a boxed table pads every row to one shared width. The
        // Pivoted key-value layout draws no box and is deliberately ragged
        // — a suppressed header renders blank, a record rule and a
        // `Label: Value` row differ — so holding it to a single width
        // would be asserting a property it never had.
        if !m.boxed {
            continue;
        }
        let Some(row) = painted.cells.get(i) else {
            continue;
        };
        let width: usize = row.iter().map(|c| c.width as usize).sum();
        match first_of_group.get(&group) {
            Some(&(first_row, first_width)) if first_width != width => {
                return Some(Violation::new(
                    "TABLE-ROW-WIDTH",
                    format!(
                        "table_group {group}: row {i} has summed width {width}, but row \
                         {first_row} (same group) has width {first_width}"
                    ),
                ));
            }
            Some(_) => {}
            None => {
                first_of_group.insert(group, (i, width));
            }
        }
    }
    None
}

/// `TABLE-SYNTHETIC-DECORATIVE` (L0, sampled per G19; plan WP5.S4) — every
/// cell of a row whose `RowMeta.synthetic` is `true` carries no
/// `buf_offset`: a synthesised border row has no source line at all
/// (`DisplaySnapshot::expand_tables`'s docs), so none of its cells may
/// claim a real buffer byte.
///
/// Active-document-switch-safe: L0, single `Painted`'s `cells`/`row_meta`.
pub fn table_synthetic_decorative(painted: &Painted) -> Option<Violation> {
    for (i, m) in painted.row_meta.iter().enumerate() {
        if !m.synthetic {
            continue;
        }
        let Some(row) = painted.cells.get(i) else {
            continue;
        };
        for cell in row {
            if let Some(offset) = cell.buf_offset {
                return Some(Violation::new(
                    "TABLE-SYNTHETIC-DECORATIVE",
                    format!(
                        "synthetic row {i} has a cell with buf_offset={offset} \
                         (must be decorative only)"
                    ),
                ));
            }
        }
    }
    None
}
