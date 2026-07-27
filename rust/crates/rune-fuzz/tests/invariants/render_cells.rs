//! WP6.S5 detection tests: `SYNC-IDEMPOTENT`, `CELL-OFFSET`,
//! `CELL-NO-EOL`, `CELL-ORDER`.

use rune_fuzz::invariant::{cell_no_eol, cell_offset, cell_order, sync_idempotent};

use crate::support::{base_snapshot, cell};

// ---------------------------------------------------------------------
// SYNC-IDEMPOTENT
// ---------------------------------------------------------------------

#[test]
fn sync_idempotent_detects_a_changed_second_render() {
    let before = vec![vec![cell('a', 0), cell('b', 1)]];
    let after = vec![vec![cell('a', 0), cell('x', 1)]]; // second sync_view() changed a cell
    let v = sync_idempotent(&before, 0, &after, 0)
        .expect("a changed second render must trip SYNC-IDEMPOTENT");
    assert_eq!(v.id, "SYNC-IDEMPOTENT");
}

#[test]
fn sync_idempotent_detects_a_moved_scroll_row() {
    let rows = vec![vec![cell('a', 0)]];
    let v = sync_idempotent(&rows, 0, &rows, 3)
        .expect("a second sync_view() moving scroll_row must trip SYNC-IDEMPOTENT");
    assert_eq!(v.id, "SYNC-IDEMPOTENT");
}

#[test]
fn sync_idempotent_accepts_an_unchanged_second_render() {
    let rows = vec![vec![cell('a', 0), cell('b', 1)]];
    assert_eq!(sync_idempotent(&rows, 5, &rows, 5), None);
}

// ---------------------------------------------------------------------
// CELL-OFFSET
// ---------------------------------------------------------------------

#[test]
fn cell_offset_detects_out_of_bounds_offset() {
    let mut snap = base_snapshot("abc");
    snap.cells = vec![vec![cell('a', snap.content.len() as i64 + 1)]];
    let v = cell_offset(&snap).expect("an out-of-bounds buf_offset must trip CELL-OFFSET");
    assert_eq!(v.id, "CELL-OFFSET");
}

#[test]
fn cell_offset_detects_mid_rune_offset() {
    let mut snap = base_snapshot("é"); // 2 bytes; offset 1 is mid-rune
    snap.cells = vec![vec![cell('é', 1)]];
    let v = cell_offset(&snap).expect("a mid-rune buf_offset must trip CELL-OFFSET");
    assert_eq!(v.id, "CELL-OFFSET");
}

#[test]
fn cell_offset_accepts_sentinel_and_valid_boundaries() {
    let mut snap = base_snapshot("abc");
    snap.cells = vec![vec![cell('a', 0), cell(' ', -1), cell('c', 2)]];
    assert_eq!(cell_offset(&snap), None);
}

// ---------------------------------------------------------------------
// CELL-NO-EOL
// ---------------------------------------------------------------------

#[test]
fn cell_no_eol_detects_a_newline_cell() {
    let mut snap = base_snapshot("a\nb");
    snap.cells = vec![vec![cell('a', 0), cell('\n', 1)]];
    let v = cell_no_eol(&snap).expect("a cell carrying '\\n' must trip CELL-NO-EOL");
    assert_eq!(v.id, "CELL-NO-EOL");
}

#[test]
fn cell_no_eol_detects_a_carriage_return_cell() {
    let mut snap = base_snapshot("a\rb");
    snap.cells = vec![vec![cell('a', 0), cell('\r', 1)]];
    let v = cell_no_eol(&snap).expect("a cell carrying '\\r' must trip CELL-NO-EOL");
    assert_eq!(v.id, "CELL-NO-EOL");
}

#[test]
fn cell_no_eol_accepts_ordinary_chars() {
    let mut snap = base_snapshot("ab");
    snap.cells = vec![vec![cell('a', 0), cell('b', 1)]];
    assert_eq!(cell_no_eol(&snap), None);
}

// ---------------------------------------------------------------------
// CELL-ORDER
// ---------------------------------------------------------------------

#[test]
fn cell_order_detects_backwards_offsets() {
    let mut snap = base_snapshot("abc");
    snap.cells = vec![vec![cell('a', 2), cell('b', 0)]];
    let v = cell_order(&snap).expect("backwards buf_offsets within a row must trip CELL-ORDER");
    assert_eq!(v.id, "CELL-ORDER");
}

#[test]
fn cell_order_accepts_non_decreasing_offsets_and_skips_sentinels() {
    let mut snap = base_snapshot("abc");
    snap.cells = vec![vec![
        cell('a', 0),
        cell(' ', -1),
        cell('b', 1),
        cell('c', 2),
    ]];
    assert_eq!(cell_order(&snap), None);
}
