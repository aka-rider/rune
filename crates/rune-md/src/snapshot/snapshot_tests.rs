#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use rune_syntax::SyntaxLine;
use rune_syntax::wrap::WrapMap;

#[test]
fn display_snapshot_row_count_matches_wrap_in_the_no_table_case() {
    // Two empty lines wrap to exactly 2 rows (each empty `SyntaxLine`
    // gets its own single, empty segment — see `WrapMap::wrap_line`'s
    // `line.spans.is_empty()` case). Pin that concrete count instead of
    // comparing `DisplaySnapshot::from_wrap`'s output back to
    // `wrap.total_rows()` — the very value it copies — which passes
    // trivially by construction regardless of what `total_rows()`
    // actually is.
    let lines = vec![SyntaxLine::default(), SyntaxLine::default()];
    let wrap = WrapMap::new(80).sync("", &lines);
    assert_eq!(wrap.total_rows(), 2);
    let display = DisplaySnapshot::from_wrap(&wrap);
    assert_eq!(display.total_rows(), 2);
    // No table lines: `expand_tables` must be a true no-op.
    let expanded = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
    assert_eq!(expanded.total_rows(), 2);
    for r in expanded.rows() {
        assert!(!r.synthetic);
    }
}

/// WP3.S7: a 4-line table (header, separator, two body rows — no
/// trailing blank line, so wrap has exactly 4 rows, one per source
/// line) expands to 7 display rows: a synthesised `┌┬┐` before the
/// header, the header/separator/two body rows unchanged, a synthesised
/// `├┼┤` between the two body rows (they're different source lines,
/// both `Body` — the case with no real separator line of their own),
/// and a synthesised `└┴┘` after the last body row.
#[test]
fn four_line_table_expands_to_seven_display_rows() {
    let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
    let blocks = crate::parse::parse(content);
    let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
    assert_eq!(lines.len(), 4, "fixture must have no trailing blank line");
    let wrap = WrapMap::new(80).sync(content, &lines);
    assert_eq!(wrap.total_rows(), 4);

    let display = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
    assert_eq!(display.total_rows(), 7);

    let synthetic_count = display.rows().iter().filter(|r| r.synthetic).count();
    assert_eq!(synthetic_count, 3, "top + bottom + one inter-row border");

    assert!(display.rows().first().is_some_and(|r| r.synthetic));
    assert!(display.rows().last().is_some_and(|r| r.synthetic));

    // The middle border must sit between the two BODY rows specifically
    // (wrap rows 2 and 3), borrowing the LATTER's wrap_row per a
    // leading border's own convention (see `expand_tables`'s docs) —
    // not between the separator and the first body row, which a
    // `prev_role == Body` check flipped to `!=` would misplace it at
    // instead (same total row/synthetic COUNT either way, so only the
    // exact wrap_row pins the bug).
    let middle = display
        .rows()
        .iter()
        .filter(|r| r.synthetic)
        .nth(1)
        .expect("a middle border between the two body rows");
    assert_eq!(middle.wrap_row, 3);
}

/// WP4.S3: the new `image` field must default to `None` for every row
/// neither the image producer nor `expand_images` touches — both the
/// real (non-synthetic) rows `from_wrap` builds and the synthetic
/// table-border rows `expand_tables` inserts.
#[test]
fn display_row_image_field_defaults_to_none_for_non_image_rows() {
    let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
    let blocks = crate::parse::parse(content);
    let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
    let wrap = WrapMap::new(80).sync(content, &lines);
    let display = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
    assert!(display.rows().iter().any(|r| r.synthetic));
    assert!(display.rows().iter().any(|r| !r.synthetic));
    for r in display.rows() {
        assert_eq!(r.image, None, "row should have no image marker");
    }
}

/// WP4.S2: `image_rows` reserves exactly `n` rows (floored at 1), every
/// one synthetic and carrying an `ImageRowRef` naming its own 0-based
/// row index and the given width.
#[test]
fn image_rows_reserves_n_synthetic_rows() {
    let display = DisplaySnapshot::image_rows(4, 12);
    assert_eq!(display.total_rows(), 4);
    for (i, row) in display.rows().iter().enumerate() {
        assert!(row.synthetic);
        assert_eq!(
            row.image,
            Some(ImageRowRef {
                row: i,
                width: 12,
                target: None,
            })
        );
    }
}

/// `n` is floored at 1 — an image document must never be left with zero
/// rows to scroll or render into.
#[test]
fn image_rows_floors_at_one() {
    let display = DisplaySnapshot::image_rows(0, 5);
    assert_eq!(display.total_rows(), 1);
}

/// `display_to_wrap(wrap_to_display(r)) == r` for every wrap row, and
/// every synthetic row's `wrap_row` equals an adjacent content row's own
/// — the round-trip and adjacency assertions.
#[test]
fn wrap_display_round_trip_and_synthetic_adjacency() {
    let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
    let blocks = crate::parse::parse(content);
    let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
    let wrap = WrapMap::new(80).sync(content, &lines);
    let display = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
    assert_round_trip_and_synthetic_adjacency(&wrap, &display);
}

fn four_line_table_display() -> DisplaySnapshot {
    let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";
    let blocks = crate::parse::parse(content);
    let (lines, _syntax) = crate::emit::emit(content, &blocks, 80);
    let wrap = WrapMap::new(80).sync(content, &lines);
    DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap)
}

/// An out-of-range display row clamps to the LAST row (`rows.len() -
/// 1`), not one past it — pinned with a fixture whose last row's
/// `wrap_row` (3) differs from every other row's, so a wrong clamp
/// bound reads a wrong (or out-of-bounds, defaulting to 0) row instead.
#[test]
fn display_to_wrap_clamps_to_the_last_row() {
    let display = four_line_table_display();
    assert_eq!(display.total_rows(), 7);
    assert_eq!(display.rows().last().unwrap().wrap_row, 3);
    assert_eq!(display.display_to_wrap(DisplayRow(1000)), WrapRow(3));
}

/// Same clamp, the `wrap_to_display` side: an out-of-range wrap row
/// clamps to `wrap_to_display.len() - 1`, landing on the LAST real
/// row's own display index (5, the second body row) — not one past it.
#[test]
fn wrap_to_display_clamps_to_the_last_wrap_row() {
    let display = four_line_table_display();
    assert_eq!(display.wrap_to_display(WrapRow(1000)), DisplayRow(5));
}

/// `line_start_of` reads a content row's own first span's range start
/// directly — pinned against a value that is neither of the constant
/// fallbacks (`0`/`1`) a broken producer might return instead.
#[test]
fn line_start_of_reads_the_first_spans_range_start() {
    let span = SyntaxSpan::substituted(0, String::new(), table_border_scope(), 42..42);
    let row = SnapshotRow {
        spans: vec![span],
        wrap_row: 0,
        synthetic: false,
        decor: None,
        image: None,
    };
    assert_eq!(line_start_of(&row), 42);
}
