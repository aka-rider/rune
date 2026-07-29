//! AST -> `TableM` construction (plan WP1.S3). comrak's `table` extension
//! gives real per-cell sourcepos, so escaped pipes, pipes inside inline
//! code, and GFM's own ragged-row padding/truncation are correct for free —
//! unlike Go's `strings.Split(line, "|")` line-splitting. Reuses
//! `super::inline::build_inlines` for cell content instead of writing a
//! second inline builder.

use super::inline::build_inlines;
use super::{ScanHint, line_at, node_range, per_line_content};
use crate::element::block::Block;
use crate::element::table::{TableAlign, TableCellM, TableM, TableRowM};
use comrak::nodes::{AstNode, NodeValue, TableAlignment};
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

fn to_align(a: TableAlignment) -> TableAlign {
    match a {
        TableAlignment::None => TableAlign::None,
        TableAlignment::Left => TableAlign::Left,
        TableAlignment::Center => TableAlign::Center,
        TableAlignment::Right => TableAlign::Right,
    }
}

/// `None` for a non-`Table` node or a table with no rows, so the caller
/// (`parse::block::build_block`) falls back to the existing `Verbatim`
/// construction (§1.3: degrade, never panic).
pub(super) fn build_table<'a>(
    content: &str,
    starts: &[usize],
    node: &'a AstNode<'a>,
    hint: &ScanHint,
    range: ByteRange,
) -> Option<Block> {
    // The alignment vec is read and cloned out, dropping the borrow before
    // any recursive `build_inlines` call re-borrows a child node's own
    // `RefCell` — the same reentrancy hazard `parse::block::build_block`'s
    // `clone_kind_tag` exists to avoid (comrak's `Ast` is a `RefCell`; a
    // live borrow held across a call that borrows a DIFFERENT node is fine,
    // but there is no reason to hold this one open past what it's used for).
    let aligns: Vec<TableAlign> = {
        let data = node.data.borrow();
        let NodeValue::Table(t) = &data.value else {
            return None;
        };
        t.alignments.iter().copied().map(to_align).collect()
    };

    let mut rows: Vec<TableRowM> = Vec::new();
    let mut header_line: Option<usize> = None;

    for row_node in node.children() {
        let is_header = match row_node.data.borrow().value {
            NodeValue::TableRow(h) => h,
            _ => continue,
        };
        let row_range = node_range(content, starts, row_node);
        let row_line = line_at(starts, row_range.start);
        if is_header && header_line.is_none() {
            header_line = Some(row_line);
        }

        let mut cells: Vec<TableCellM> = Vec::new();
        for cell_node in row_node.children() {
            if !matches!(cell_node.data.borrow().value, NodeValue::TableCell) {
                continue;
            }
            let cell_range = node_range(content, starts, cell_node);
            let inlines = build_inlines(content, starts, cell_node, hint);
            cells.push(TableCellM {
                range: cell_range,
                inlines,
            });
        }

        rows.push(TableRowM {
            line: row_line,
            is_header,
            cells,
        });
    }

    if rows.is_empty() {
        return None;
    }

    let first_line = line_at(starts, range.start);
    let last_line = line_at(starts, range.end.saturating_sub(1).max(range.start));
    // The `|---|---|` delimiter row has no comrak node at all: derive it as
    // the line right after the header row, clamped so a malformed/truncated
    // table can never point past its own last line.
    let sep_line = header_line
        .unwrap_or(first_line)
        .saturating_add(1)
        .min(last_line);

    // Every row and the delimiter must occupy a DISTINCT buffer line — a
    // defensive backstop, not something well-formed GFM tables ever
    // violate: were it ever to happen (a comrak sourcepos quirk this
    // crate hasn't seen yet), a real body row and the synthetic delimiter
    // would land on the same line and render as one display row carrying
    // two rows' worth of cells, with buffer offsets that run backwards
    // mid-row. Rather than let a `TableM` exist in that state, decline it
    // here: the caller falls back to raw passthrough, so the user's bytes
    // still reach the screen verbatim (§1.3 — unknown or undecidable
    // syntax degrades to visible raw text, never lost). This only ever
    // guards the table's OWN rows against each other; a SIBLING block
    // sharing a row's buffer line is a different hazard, ruled out
    // upstream instead — CommonMark's own block parser assigns each
    // physical line to exactly one block, and that invariant only holds
    // for comrak's purposes when its line count agrees with `starts`
    // (see `parse::cr_shadow`'s docs for the one case where it wouldn't).

    // The table's range must begin exactly where its first line's content
    // begins — at the line start, or after whatever container prefix the
    // scan hint accounts for (a blockquote's `"> "`, a list item's marker).
    // An UNEXPLAINED mid-line start means raw leading whitespace, and there
    // comrak reports the cell sourcepos of every LATER row shifted one byte
    // right: a header written `" Name | Age |"` yields body cells
    // `"Alice |"`/`"30 |"` instead of `" Alice "`/`" 30 "`, so every cell
    // would render missing its first character while the skipped byte leaks
    // back as raw text. Decline rather than render the user's words wrongly
    // — the caller falls back to raw passthrough and the bytes reach the
    // screen verbatim (§1.3, and the prime directive: protect the words
    // above rendering them prettily). A container-explained start is NOT
    // affected and still renders, so this doesn't disable tables in
    // blockquotes or list items.
    let first = line_at(starts, range.start);
    if range.start != hint.start_for_line(starts, first) {
        return None;
    }
    let mut claimed: Vec<usize> = rows.iter().map(|r| r.line).collect();
    claimed.push(sep_line);
    claimed.sort_unstable();
    let claimed_total = claimed.len();
    claimed.dedup();
    if claimed.len() != claimed_total {
        return None;
    }

    let content_lines = per_line_content(content, starts, range, hint);

    Some(Block::Table(TableM {
        sm: RevealSm::new(RevealState::Rendered),
        range,
        aligns,
        rows,
        sep_line,
        first_line,
        last_line,
        content_lines,
    }))
}
