//! Cell-model invariants: `CELL-OFFSET`/`CELL-NO-EOL`/`CELL-ORDER` (Go
//! `R4`/`R5`, `R8`, `R3`) over `Snapshot.cells`, plus the pure comparator
//! half of `SYNC-IDEMPOTENT` (§8 "Render Purity", `CONSTITUTION.md:238`).
//!
//! `SYNC-IDEMPOTENT` itself needs a SECOND live `app.sync_view()` call with
//! no intervening message — it is a comparison of two consecutive renders
//! of the same settled state, not a property of one `Snapshot` — so
//! `driver.rs` drives the two `render::build_rows` calls directly against
//! `&mut App` (G6 proves this is a genuine fixpoint: `Editor::view` never
//! reads `viewport.scroll_row`, and `Viewport::scroll_to_row` converges in
//! one call) and hands the results to `sync_idempotent` below, which is
//! the actual pure, hand-testable assertion.

use rune_tui::render::Cell;

use super::Violation;
use crate::snapshot::Snapshot;

/// `SYNC-IDEMPOTENT` — `rows_before`/`scroll_before` are captured
/// immediately before a second, message-free `app.sync_view()`;
/// `rows_after`/`scroll_after` immediately after. A firing here is a real
/// non-settling scroll or a non-memoized parse, never a false positive
/// (G6).
pub fn sync_idempotent(
    rows_before: &[Vec<Cell>],
    scroll_before: usize,
    rows_after: &[Vec<Cell>],
    scroll_after: usize,
) -> Option<Violation> {
    if rows_before != rows_after {
        return Some(Violation {
            id: "SYNC-IDEMPOTENT",
            message: format!(
                "a second sync_view() with no intervening message changed the rendered rows \
                 ({} rows before, {} rows after)",
                rows_before.len(),
                rows_after.len()
            ),
        });
    }
    if scroll_before != scroll_after {
        return Some(Violation {
            id: "SYNC-IDEMPOTENT",
            message: format!(
                "a second sync_view() with no intervening message moved scroll_row: \
                 {scroll_before} -> {scroll_after}"
            ),
        });
    }
    None
}

/// `CELL-OFFSET` (L0, sampled per G19; Go `R4`/`R5`) — every
/// `Cell.buf_offset` is `-1` or a valid, in-bounds, char-boundary byte
/// offset into `content`; a non-negative offset implies `width >= 1` (a
/// real buffer byte always renders as at least one cell).
pub fn cell_offset(snap: &Snapshot) -> Option<Violation> {
    for row in &snap.cells {
        for cell in row {
            if cell.buf_offset == -1 {
                continue;
            }
            if cell.buf_offset < 0 {
                return Some(Violation {
                    id: "CELL-OFFSET",
                    message: format!(
                        "cell buf_offset={} is negative but not the -1 sentinel",
                        cell.buf_offset
                    ),
                });
            }
            let offset = cell.buf_offset as usize;
            if offset > snap.content.len() || !snap.content.is_char_boundary(offset) {
                return Some(Violation {
                    id: "CELL-OFFSET",
                    message: format!(
                        "cell buf_offset={offset} is out of bounds or not a char boundary \
                         (content.len()={})",
                        snap.content.len()
                    ),
                });
            }
            if cell.width == 0 {
                return Some(Violation {
                    id: "CELL-OFFSET",
                    message: format!("cell at buf_offset={offset} has width=0"),
                });
            }
        }
    }
    None
}

/// `CELL-NO-EOL` (L0, sampled per G19; Go `R8`) — no cell's `ch` is `\n` or
/// `\r`: those bytes carry zero display width and must never reach a
/// rendered cell (`push_char_cells` drops them, `render.rs`).
pub fn cell_no_eol(snap: &Snapshot) -> Option<Violation> {
    for row in &snap.cells {
        for cell in row {
            if cell.ch == '\n' || cell.ch == '\r' {
                return Some(Violation {
                    id: "CELL-NO-EOL",
                    message: format!(
                        "cell carries an EOL char {:?} at buf_offset={}",
                        cell.ch, cell.buf_offset
                    ),
                });
            }
        }
    }
    None
}

/// `CELL-ORDER` (L0, sampled per G19; Go `R3`) — within each row, cells
/// with a real (non-negative) `buf_offset` are non-decreasing left to
/// right.
pub fn cell_order(snap: &Snapshot) -> Option<Violation> {
    for row in &snap.cells {
        let mut last: Option<i64> = None;
        for cell in row {
            if cell.buf_offset < 0 {
                continue;
            }
            if let Some(prev_offset) = last
                && cell.buf_offset < prev_offset
            {
                return Some(Violation {
                    id: "CELL-ORDER",
                    message: format!(
                        "row cell buf_offsets go backwards: {prev_offset} then {}",
                        cell.buf_offset
                    ),
                });
            }
            last = Some(cell.buf_offset);
        }
    }
    None
}
