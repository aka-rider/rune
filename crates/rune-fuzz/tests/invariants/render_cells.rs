//! WP6.S5 detection tests: `SYNC-IDEMPOTENT`, `CELL-OFFSET`,
//! `CELL-NO-EOL`, `CELL-ORDER`. WP5.S5 adds `TABLE-ROW-WIDTH` and
//! `TABLE-SYNTHETIC-DECORATIVE`.

use rune_fuzz::invariant::{
    cell_no_eol, cell_offset, cell_order, sync_idempotent, table_row_width,
    table_synthetic_decorative,
};

use crate::support::{base_snapshot, cell, cell_w, meta};

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

// ---------------------------------------------------------------------
// TABLE-ROW-WIDTH
// ---------------------------------------------------------------------

#[test]
fn table_row_width_detects_a_disagreeing_row_in_the_same_group() {
    let mut snap = base_snapshot("| a | b |\n| - | - |\n| c | d |\n");
    // Same table_group (0): row 0 sums to width 4, row 1 to width 3 — a
    // border/content row whose width disagrees with the rest of its own
    // table, exactly the defect class this invariant exists to catch.
    snap.cells = vec![
        vec![cell_w('x', -1, 2), cell_w('y', -1, 2)],
        vec![cell_w('x', -1, 2), cell_w('y', -1, 1)],
    ];
    snap.row_meta = vec![meta(true, Some(0)), meta(false, Some(0))];
    let v = table_row_width(&snap).expect(
        "a row whose summed width disagrees with its table_group must trip TABLE-ROW-WIDTH",
    );
    assert_eq!(v.id, "TABLE-ROW-WIDTH");
}

#[test]
fn table_row_width_accepts_equal_widths_within_a_group_and_ignores_other_groups() {
    let mut snap = base_snapshot("| a | b |\n| - | - |\n| c | d |\n\n| e |\n| - |\n| f |\n");
    // Group 0: two rows, both width 4. Group 1: one row, width 3 — a
    // DIFFERENT table, allowed to differ from group 0 freely. A `None`
    // group (plain prose) is ignored entirely.
    snap.cells = vec![
        vec![cell_w('x', -1, 2), cell_w('y', -1, 2)],
        vec![cell_w('x', -1, 2), cell_w('y', -1, 2)],
        vec![cell_w('z', -1, 3)],
        vec![cell_w('p', 0, 1)],
    ];
    snap.row_meta = vec![
        meta(true, Some(0)),
        meta(false, Some(0)),
        meta(true, Some(1)),
        meta(false, None),
    ];
    assert_eq!(table_row_width(&snap), None);
}

// ---------------------------------------------------------------------
// TABLE-SYNTHETIC-DECORATIVE
// ---------------------------------------------------------------------

#[test]
fn table_synthetic_decorative_detects_a_real_offset_on_a_border_row() {
    let mut snap = base_snapshot("| a | b |\n| - | - |\n");
    snap.cells = vec![vec![cell('┌', -1), cell('a', 3)]]; // a border row must never carry a real byte
    snap.row_meta = vec![meta(true, Some(0))];
    let v = table_synthetic_decorative(&snap)
        .expect("a synthetic row cell with a real buf_offset must trip TABLE-SYNTHETIC-DECORATIVE");
    assert_eq!(v.id, "TABLE-SYNTHETIC-DECORATIVE");
}

#[test]
fn table_synthetic_decorative_accepts_all_sentinel_borders_and_ignores_content_rows() {
    let mut snap = base_snapshot("| a | b |\n| - | - |\n");
    snap.cells = vec![
        vec![cell('┌', -1), cell('─', -1), cell('┐', -1)],
        vec![cell('a', 2), cell(' ', -1), cell('b', 6)], // non-synthetic: real offsets are fine
    ];
    snap.row_meta = vec![meta(true, Some(0)), meta(false, Some(0))];
    assert_eq!(table_synthetic_decorative(&snap), None);
}
