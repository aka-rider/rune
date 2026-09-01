//! WP6.S5 detection tests: `SYNC-IDEMPOTENT`, `CELL-OFFSET`,
//! `CELL-NO-EOL`, `CELL-ORDER`. WP5.S5 adds `TABLE-ROW-WIDTH` and
//! `TABLE-SYNTHETIC-DECORATIVE`.

use rune_fuzz::invariant::{
    cell_no_eol, cell_offset, cell_order, cur_cell_sync, cur_no_caret_hidden, sync_idempotent,
    table_row_width, table_synthetic_decorative,
};
use rune_syntax::element::ByteRange;

use crate::support::{
    base_snapshot, cell, cell_w, collapsed_cursor, meta, meta_unboxed, reversed_cell,
};

// ---------------------------------------------------------------------
// SYNC-IDEMPOTENT
// ---------------------------------------------------------------------

#[test]
fn sync_idempotent_detects_a_changed_second_render() {
    let before = vec![vec![cell('a', Some(0)), cell('b', Some(1))]];
    let after = vec![vec![cell('a', Some(0)), cell('x', Some(1))]]; // second sync_view() changed a cell
    let v = sync_idempotent(&before, 0, &after, 0)
        .expect("a changed second render must trip SYNC-IDEMPOTENT");
    assert_eq!(v.id, "SYNC-IDEMPOTENT");
}

#[test]
fn sync_idempotent_detects_a_moved_scroll_row() {
    let rows = vec![vec![cell('a', Some(0))]];
    let v = sync_idempotent(&rows, 0, &rows, 3)
        .expect("a second sync_view() moving scroll_row must trip SYNC-IDEMPOTENT");
    assert_eq!(v.id, "SYNC-IDEMPOTENT");
}

#[test]
fn sync_idempotent_accepts_an_unchanged_second_render() {
    let rows = vec![vec![cell('a', Some(0)), cell('b', Some(1))]];
    assert_eq!(sync_idempotent(&rows, 5, &rows, 5), None);
}

// ---------------------------------------------------------------------
// CELL-OFFSET
// ---------------------------------------------------------------------

#[test]
fn cell_offset_detects_out_of_bounds_offset() {
    let mut snap = base_snapshot("abc");
    snap.painted.cells = vec![vec![cell('a', Some(snap.content.len() as u32 + 1))]];
    let v = cell_offset(&snap.painted).expect("an out-of-bounds buf_offset must trip CELL-OFFSET");
    assert_eq!(v.id, "CELL-OFFSET");
}

#[test]
fn cell_offset_detects_mid_rune_offset() {
    let mut snap = base_snapshot("é"); // 2 bytes; offset 1 is mid-rune
    snap.painted.cells = vec![vec![cell('é', Some(1))]];
    let v = cell_offset(&snap.painted).expect("a mid-rune buf_offset must trip CELL-OFFSET");
    assert_eq!(v.id, "CELL-OFFSET");
}

#[test]
fn cell_offset_accepts_sentinel_and_valid_boundaries() {
    let mut snap = base_snapshot("abc");
    snap.painted.cells = vec![vec![
        cell('a', Some(0)),
        cell(' ', None),
        cell('c', Some(2)),
    ]];
    assert_eq!(cell_offset(&snap.painted), None);
}

/// A real (non-decorative) cell whose declared `width` disagrees with what
/// ratatui itself derives for its own `text` is a producer bug — 'a' is
/// ordinary width-1 text, so declaring it width 0 must still trip
/// `CELL-OFFSET`, exactly as it always did.
#[test]
fn cell_offset_detects_width_zero_on_an_ordinary_char() {
    let mut snap = base_snapshot("abc");
    snap.painted.cells = vec![vec![cell_w('a', Some(0), 0)]];
    let v = cell_offset(&snap.painted).expect("width=0 on ordinary text must trip CELL-OFFSET");
    assert_eq!(v.id, "CELL-OFFSET");
}

/// The decided policy's own carve-out: a real cell carrying a LONE
/// zero-width rune (`rune_syntax::wrap::grapheme_width`'s doc — a bare
/// combining mark, a stray ZWJ, a lone variation selector, a lone
/// zero-width space) legitimately derives width 0, matching ratatui's own
/// `cell_width()` for that same symbol — this must NOT trip `CELL-OFFSET`.
#[test]
fn cell_offset_accepts_width_zero_matching_ratatuis_own_derivation() {
    let mut snap = base_snapshot("\u{200d}bc");
    snap.painted.cells = vec![vec![cell_w('\u{200d}', Some(0), 0)]];
    assert_eq!(cell_offset(&snap.painted), None);
}

// ---------------------------------------------------------------------
// CELL-NO-EOL
// ---------------------------------------------------------------------

#[test]
fn cell_no_eol_detects_a_newline_cell() {
    let mut snap = base_snapshot("a\nb");
    snap.painted.cells = vec![vec![cell('a', Some(0)), cell('\n', Some(1))]];
    let v = cell_no_eol(&snap.painted).expect("a cell carrying '\\n' must trip CELL-NO-EOL");
    assert_eq!(v.id, "CELL-NO-EOL");
}

#[test]
fn cell_no_eol_detects_a_carriage_return_cell() {
    let mut snap = base_snapshot("a\rb");
    snap.painted.cells = vec![vec![cell('a', Some(0)), cell('\r', Some(1))]];
    let v = cell_no_eol(&snap.painted).expect("a cell carrying '\\r' must trip CELL-NO-EOL");
    assert_eq!(v.id, "CELL-NO-EOL");
}

#[test]
fn cell_no_eol_accepts_ordinary_chars() {
    let mut snap = base_snapshot("ab");
    snap.painted.cells = vec![vec![cell('a', Some(0)), cell('b', Some(1))]];
    assert_eq!(cell_no_eol(&snap.painted), None);
}

// ---------------------------------------------------------------------
// CELL-ORDER
// ---------------------------------------------------------------------

#[test]
fn cell_order_detects_backwards_offsets() {
    let mut snap = base_snapshot("abc");
    snap.painted.cells = vec![vec![cell('a', Some(2)), cell('b', Some(0))]];
    let v =
        cell_order(&snap.painted).expect("backwards buf_offsets within a row must trip CELL-ORDER");
    assert_eq!(v.id, "CELL-ORDER");
}

#[test]
fn cell_order_accepts_non_decreasing_offsets_and_skips_sentinels() {
    let mut snap = base_snapshot("abc");
    snap.painted.cells = vec![vec![
        cell('a', Some(0)),
        cell(' ', None),
        cell('b', Some(1)),
        cell('c', Some(2)),
    ]];
    assert_eq!(cell_order(&snap.painted), None);
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
    snap.painted.cells = vec![
        vec![cell_w('x', None, 2), cell_w('y', None, 2)],
        vec![cell_w('x', None, 2), cell_w('y', None, 1)],
    ];
    snap.painted.row_meta = vec![meta(true, Some(0)), meta(false, Some(0))];
    let v = table_row_width(&snap.painted).expect(
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
    snap.painted.cells = vec![
        vec![cell_w('x', None, 2), cell_w('y', None, 2)],
        vec![cell_w('x', None, 2), cell_w('y', None, 2)],
        vec![cell_w('z', None, 3)],
        vec![cell_w('p', Some(0), 1)],
    ];
    snap.painted.row_meta = vec![
        meta(true, Some(0)),
        meta(false, Some(0)),
        meta(true, Some(1)),
        meta(false, None),
    ];
    assert_eq!(table_row_width(&snap.painted), None);
}

// ---------------------------------------------------------------------
// TABLE-SYNTHETIC-DECORATIVE
// ---------------------------------------------------------------------

#[test]
fn table_synthetic_decorative_detects_a_real_offset_on_a_border_row() {
    let mut snap = base_snapshot("| a | b |\n| - | - |\n");
    snap.painted.cells = vec![vec![cell('┌', None), cell('a', Some(3))]]; // a border row must never carry a real byte
    snap.painted.row_meta = vec![meta(true, Some(0))];
    let v = table_synthetic_decorative(&snap.painted)
        .expect("a synthetic row cell with a real buf_offset must trip TABLE-SYNTHETIC-DECORATIVE");
    assert_eq!(v.id, "TABLE-SYNTHETIC-DECORATIVE");
}

#[test]
fn table_synthetic_decorative_accepts_all_sentinel_borders_and_ignores_content_rows() {
    let mut snap = base_snapshot("| a | b |\n| - | - |\n");
    snap.painted.cells = vec![
        vec![cell('┌', None), cell('─', None), cell('┐', None)],
        vec![cell('a', Some(2)), cell(' ', None), cell('b', Some(6))], // non-synthetic: real offsets are fine
    ];
    snap.painted.row_meta = vec![meta(true, Some(0)), meta(false, Some(0))];
    assert_eq!(table_synthetic_decorative(&snap.painted), None);
}

/// A Pivoted table draws no box and is deliberately ragged — a suppressed
/// header renders blank while a `Label: Value` row does not — so the
/// equal-width expectation must not be applied to it. Without the `boxed`
/// scoping this input trips the invariant on a table that is rendering
/// exactly as intended (caught by the session fuzzer at a 7-column width,
/// which is narrow enough to force the Pivoted layout).
#[test]
fn table_row_width_ignores_a_ragged_unboxed_pivot_group() {
    let mut snap = base_snapshot("| a | b |\n| - | - |\n| c | d |\n");
    snap.painted.cells = vec![vec![], vec![cell_w('x', None, 2), cell_w('y', None, 2)]];
    snap.painted.row_meta = vec![meta_unboxed(Some(0)), meta_unboxed(Some(0))];
    assert!(
        table_row_width(&snap.painted).is_none(),
        "an unboxed (Pivoted) table's ragged rows must not trip TABLE-ROW-WIDTH"
    );
}

// ---------------------------------------------------------------------
// CUR-NO-CARET-HIDDEN
// ---------------------------------------------------------------------

#[test]
fn cur_no_caret_hidden_detects_a_reversed_cell_while_hidden() {
    let mut snap = base_snapshot("abc");
    snap.painted.caret_visible = false;
    snap.painted.cells = vec![vec![cell('a', Some(0)), reversed_cell('b', Some(1))]];
    let v = cur_no_caret_hidden(&snap.painted)
        .expect("a REVERSED cell while caret_visible=false must trip CUR-NO-CARET-HIDDEN");
    assert_eq!(v.id, "CUR-NO-CARET-HIDDEN");
}

#[test]
fn cur_no_caret_hidden_accepts_the_focused_reading_link_while_hidden() {
    let mut snap = base_snapshot("abc");
    snap.painted.caret_visible = false;
    snap.painted.reading_link_focus = Some(ByteRange::new(1, 3));
    snap.painted.cells = vec![vec![cell('a', Some(0)), reversed_cell('b', Some(1))]];
    assert_eq!(cur_no_caret_hidden(&snap.painted), None);
}

#[test]
fn cur_no_caret_hidden_detects_a_reversed_cell_outside_the_focused_reading_link() {
    let mut snap = base_snapshot("abc");
    snap.painted.caret_visible = false;
    snap.painted.reading_link_focus = Some(ByteRange::new(2, 3));
    snap.painted.cells = vec![vec![cell('a', Some(0)), reversed_cell('b', Some(1))]];
    let v = cur_no_caret_hidden(&snap.painted)
        .expect("a REVERSED cell outside the focused reading link must trip CUR-NO-CARET-HIDDEN");
    assert_eq!(v.id, "CUR-NO-CARET-HIDDEN");
}

#[test]
fn cur_no_caret_hidden_accepts_reversed_cells_while_visible() {
    let mut snap = base_snapshot("abc");
    snap.painted.caret_visible = true;
    snap.painted.cells = vec![vec![cell('a', Some(0)), reversed_cell('b', Some(1))]];
    assert_eq!(cur_no_caret_hidden(&snap.painted), None);
}

#[test]
fn cur_no_caret_hidden_accepts_no_reversed_cells_while_hidden() {
    let mut snap = base_snapshot("abc");
    snap.painted.caret_visible = false;
    snap.painted.cells = vec![vec![cell('a', Some(0)), cell('b', Some(1))]];
    assert_eq!(cur_no_caret_hidden(&snap.painted), None);
}

// ---------------------------------------------------------------------
// CUR-CELL-SYNC
// ---------------------------------------------------------------------

#[test]
fn cur_cell_sync_accepts_the_caret_on_its_own_logical_byte() {
    let mut snap = base_snapshot("abc");
    snap.painted.cursors = vec![collapsed_cursor(1, 0)];
    snap.painted.caret_visible = true;
    snap.painted.cells = vec![vec![
        reversed_cell('a', Some(0)),
        cell('b', Some(1)),
        cell('c', Some(2)),
    ]];
    assert_eq!(cur_cell_sync(&snap.painted), None);
}

#[test]
fn cur_cell_sync_detects_the_caret_painted_on_a_different_offset() {
    let mut snap = base_snapshot("abc");
    snap.painted.cursors = vec![collapsed_cursor(1, 0)];
    snap.painted.caret_visible = true;
    // Position 0 is rendered plainly, but the caret's REVERSED landed on
    // offset 1 instead — exactly the two-width-walk divergence this
    // invariant exists to catch.
    snap.painted.cells = vec![vec![
        cell('a', Some(0)),
        reversed_cell('b', Some(1)),
        cell('c', Some(2)),
    ]];
    let v = cur_cell_sync(&snap.painted)
        .expect("a caret painted off the cursor's own position must trip CUR-CELL-SYNC");
    assert_eq!(v.id, "CUR-CELL-SYNC");
}

#[test]
fn cur_cell_sync_skips_a_position_no_cell_claims() {
    let mut snap = base_snapshot("abc");
    // Position 5 is out of this row's rendered window entirely — a
    // concealed run's interior or a cursor scrolled off-viewport, neither
    // of which any cell can claim.
    snap.painted.cursors = vec![collapsed_cursor(1, 5)];
    snap.painted.caret_visible = true;
    snap.painted.cells = vec![vec![
        cell('a', Some(0)),
        cell('b', Some(1)),
        cell('c', Some(2)),
    ]];
    assert_eq!(cur_cell_sync(&snap.painted), None);
}

#[test]
fn cur_cell_sync_skips_while_caret_invisible() {
    let mut snap = base_snapshot("abc");
    snap.painted.cursors = vec![collapsed_cursor(1, 0)];
    snap.painted.caret_visible = false;
    snap.painted.cells = vec![vec![
        cell('a', Some(0)),
        cell('b', Some(1)),
        cell('c', Some(2)),
    ]];
    assert_eq!(cur_cell_sync(&snap.painted), None);
}

#[test]
fn cur_cell_sync_accepts_two_cursors_collapsed_onto_one_visible_cell() {
    let mut snap = base_snapshot("abc");
    // Cursor 2 sits inside a concealed run (position 1, claimed by no
    // cell) and visually collapses onto cursor 1's own rendered byte —
    // only cursor 1's own position needs a matching caret cell.
    snap.painted.cursors = vec![collapsed_cursor(1, 0), collapsed_cursor(2, 1)];
    snap.painted.caret_visible = true;
    snap.painted.cells = vec![vec![reversed_cell('a', Some(0)), cell('c', Some(2))]];
    assert_eq!(cur_cell_sync(&snap.painted), None);
}
