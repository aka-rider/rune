//! WP1.S6: table parses into a real `Block::Table` element machine — shape
//! coverage for alignments, header/body rows, the derived (comrak-absent)
//! delimiter line, comrak's own ragged-row padding/truncation, and cell
//! splitting on an escaped pipe / a pipe inside inline code.
//!
//! Several assertions here pin comrak's ACTUAL behaviour rather than an
//! assumed one (verified empirically against comrak 0.54.0's table row
//! scanner, `comrak-0.54.0/src/parser/table.rs`'s `row`/`scanners::table_cell`):
//! a table cell's raw sourcepos is NOT trimmed to its visible word (it
//! includes the padding spaces either side of the pipe), an autocompleted
//! cell's sourcepos is a single COLUMN position (`start.column ==
//! end.column`) which this crate's own end-inclusive `sourcepos_to_range`
//! conversion turns into a ONE-byte range, not a zero-length one, and — most
//! importantly — comrak's cell scanner does not treat a backtick run as
//! pipe-protecting at all: an unescaped `|` inside inline code splits the
//! cell exactly like a raw `strings.Split` would, either rejecting the
//! whole table (header/delimiter column-count mismatch) or silently
//! truncating a later column (see the two `pipe_inside_inline_code_*` tests
//! below).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::block::Block;
use rune_md::element::doc::DocMachine;
use rune_md::element::table::{TableAlign, TableM, TableRowShape};
use rune_md::parse::parse;
use rune_syntax::element::RevealMode;

fn only_table(content: &str) -> TableM {
    let blocks = parse(content);
    assert_eq!(blocks.len(), 1, "expected exactly one top-level block");
    let Block::Table(t) = blocks.into_iter().next().unwrap() else {
        panic!("expected Block::Table");
    };
    t
}

#[test]
fn aligns_header_and_separator_line() {
    let content = "| Name | Age |\n| :--- | ---: |\n| Alice | 30 |\n";
    let t = only_table(content);

    assert_eq!(t.aligns, vec![TableAlign::Left, TableAlign::Right]);

    // Two comrak `TableRow` nodes: the header and the one body row — the
    // `|---|---|` delimiter line has no node of its own at all (Gotcha 10),
    // so it is never counted here.
    assert_eq!(t.rows.len(), 2);
    assert!(t.rows[0].is_header);
    assert!(!t.rows[1].is_header);

    // Derived as the line right after the header row.
    assert_eq!(t.sep_line, 1);

    // Pin whatever comrak actually returns: a cell's own sourcepos is the
    // RAW span between pipes, including the padding space on both sides —
    // not trimmed to the visible word. (The cell's `inlines` — built the
    // same way a paragraph's inlines are — carry the trimmed "Alice" text
    // instead; that's a separate, already-trimmed representation.)
    let c = &t.rows[1].cells[0].range;
    assert_eq!(&content[c.start..c.end], " Alice ");
}

#[test]
fn short_row_is_padded_by_comrak_with_a_pointlike_cell() {
    // GFM autocompletes a ragged row to the table's own column count
    // (Gotcha 9). comrak gives the autocompleted cell a sourcepos whose
    // `start.column == end.column` (a single column position, verified via
    // a raw sourcepos dump: `3:5-3:5`) — but this crate's own
    // end-inclusive `sourcepos_to_range` conversion (`start = line_start +
    // (c-1)`, `end = line_start + c`) turns that into a ONE-byte range, not
    // a zero-length one. `range.start == range.end` does NOT hold; the
    // real invariant is `range.end == range.start + 1`.
    let content = "| a | b |\n| --- | --- |\n| a |\n";
    let t = only_table(content);
    let body = &t.rows[1];
    assert_eq!(body.cells.len(), 2);
    let padded = &body.cells[1].range;
    assert_eq!(padded.end, padded.start + 1);
    assert!(padded.start >= t.content_lines[2].start);
    assert!(padded.end <= t.content_lines[2].end);
}

#[test]
fn escaped_pipe_stays_inside_one_cell() {
    let content = "| a \\| b | c |\n| --- | --- |\n| x | y |\n";
    let t = only_table(content);
    let header = &t.rows[0];
    assert_eq!(header.cells.len(), 2);
    let first = &header.cells[0].range;
    assert!(content[first.start..first.end].contains('|'));
    // Anti-regression gate for the row-shape detector: a raw pipe count
    // reads three separators here (the escaped one included), which would
    // wrongly read as more cells than the header declares. The detector
    // must read comrak's own cell ranges instead, so this well-formed row
    // stays `Exact`.
    assert_eq!(header.shape, TableRowShape::Exact);
}

#[test]
fn pipe_inside_inline_code_in_header_breaks_table_recognition() {
    // Contrary to the "comrak gets this right for free" assumption: a bare
    // `|` inside inline code in the HEADER row makes the raw pipe count
    // disagree with the delimiter row's declared column count (3 raw cells
    // vs. 2), and comrak's `try_opening_header` rejects the WHOLE
    // construct rather than special-casing the code span — the entire
    // three-line span degrades to one ordinary `Paragraph`, not a table at
    // all.
    let content = "| `a|b` | c |\n| --- | --- |\n| x | y |\n";
    let blocks = parse(content);
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0], Block::Paragraph(_)));
}

#[test]
fn pipe_inside_inline_code_in_body_row_splits_the_code_span_and_drops_a_column() {
    // A code-span pipe in a BODY row doesn't reject the table (only the
    // header/delimiter column count is checked at open time), but comrak's
    // per-row cell count is truncated to the already-established column
    // count (Gotcha 9) — so the erroneous split still corrupts the row:
    // the code span itself gets cut in half across two cells, and the
    // row's real last column is silently dropped, never appearing as a
    // `TableCellM` at all.
    let content = "| a | b |\n| --- | --- |\n| `x|y` | z |\n";
    let t = only_table(content);
    let body = &t.rows[1];
    assert_eq!(body.cells.len(), 2);
    assert_eq!(
        &content[body.cells[0].range.start..body.cells[0].range.end],
        " `x"
    );
    assert_eq!(
        &content[body.cells[1].range.start..body.cells[1].range.end],
        "y` "
    );
    // The row-shape detector reads leftover source bytes past the last
    // modeled cell, not raw pipes, so it never has to know a code span was
    // involved. Here that tail is `"| z |"` (the dropped column's own text)
    // — genuinely non-whitespace content past the last cell, so this row
    // reads `Truncated` exactly like the ordinary ragged-row case: the
    // code-span pipe corrupted the split, but the detector correctly
    // reports the same real defect, a lost column, either way.
    assert_eq!(body.shape, TableRowShape::Truncated);
}

#[test]
fn long_row_with_an_extra_cell_is_classified_truncated() {
    let content = "| Name | Age |\n| ---- | --- |\n| Alice | 30 | EXTRA |\n";
    let t = only_table(content);
    let body = &t.rows[1];
    assert_eq!(body.cells.len(), 2);
    assert_eq!(body.shape, TableRowShape::Truncated);
}

#[test]
fn short_row_is_classified_padded_not_exact() {
    // `cell_is_padded` (comrak autocompletes a short row's missing cell to a
    // single-column sourcepos) feeds `any_padded` via `|=` — this is the
    // only thing that can turn a row's shape from `Exact` into `Padded`,
    // so any bug that keeps `any_padded` stuck at its initial `false`
    // (whether in `cell_is_padded` itself or in the `|=` that accumulates
    // it across a row's cells) surfaces here as a wrong `Exact`.
    let content = "| a | b |\n| --- | --- |\n| a |\n";
    let t = only_table(content);
    assert_eq!(t.rows[1].shape, TableRowShape::Padded);
}

/// `TableM::sync` folds every descendant inline's own `dirty` bit into its
/// own via `|=` — so the table's own state change (Rendered -> Revealed,
/// moving the cursor onto its lines) must stay visible in the fold even
/// though a plain `Inline::Text` cell (every cell here) always reports its
/// own `sync()` as `false`. A `&=` in that fold would let any such `false`
/// child silently swallow the table's own `true`.
#[test]
fn revealing_a_table_stays_dirty_even_though_its_text_cells_never_are() {
    let content = "before\n\n| Name | Age |\n| --- | --- |\n| Alice | 30 |\n";
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_reveal_mode(RevealMode::AtCursor);
    doc.sync_content(&buf);
    // Settle the table as Rendered with the cursor on the unrelated line
    // above it, then discard whatever dirt that settling itself produced.
    doc.sync_cursors(&buf, &CursorSet::new(0), &[]);
    doc.clear_dirty();

    let alice_at = content.find("Alice").expect("fixture has Alice");
    doc.sync_cursors(&buf, &CursorSet::new(alice_at), &[]);
    assert!(
        doc.is_dirty(),
        "moving the cursor onto the table must reveal it and mark the document dirty"
    );
}
