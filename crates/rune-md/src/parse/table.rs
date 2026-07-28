//! AST -> `TableM` construction (plan WP1.S3). comrak's `table` extension
//! gives real per-cell sourcepos, so escaped pipes, pipes inside inline
//! code, and GFM's own ragged-row padding/truncation are correct for free —
//! unlike Go's `strings.Split(line, "|")` line-splitting. Reuses
//! `super::inline::build_inlines` for cell content instead of writing a
//! second inline builder.

use super::inline::build_inlines;
use super::{LineIndex, ScanHint, line_at, node_range, per_line_content};
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
    idx: &LineIndex,
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
        let row_range = node_range(content, idx, row_node);
        let row_line = line_at(&idx.buffer, row_range.start);
        if is_header && header_line.is_none() {
            header_line = Some(row_line);
        }

        let mut cells: Vec<TableCellM> = Vec::new();
        for cell_node in row_node.children() {
            if !matches!(cell_node.data.borrow().value, NodeValue::TableCell) {
                continue;
            }
            let cell_range = node_range(content, idx, cell_node);
            let inlines = build_inlines(content, idx, cell_node, hint);
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

    let first_line = line_at(&idx.buffer, range.start);
    let last_line = line_at(&idx.buffer, range.end.saturating_sub(1).max(range.start));
    // The `|---|---|` delimiter row has no comrak node at all: derive it as
    // the line right after the header row, clamped so a malformed/truncated
    // table can never point past its own last line.
    let sep_line = header_line
        .unwrap_or(first_line)
        .saturating_add(1)
        .min(last_line);

    // Every row and the delimiter must occupy a DISTINCT buffer line. That
    // holds for well-formed markdown but not universally: row lines come from
    // the buffer's `\n`-only line index while the delimiter's position is
    // implied by comrak's CR/LF-aware one, and a lone `\r` inside a single
    // buffer line desynchronises the two — comrak sees three markdown lines
    // where the buffer has two, so a real body row and the synthetic
    // delimiter land on the same line. Rendering that collision emits one
    // display row carrying two rows' worth of cells, whose buffer offsets
    // run backwards mid-row. Rather than let a `TableM` exist in that state,
    // decline it here: the caller falls back to raw passthrough, so the
    // user's bytes still reach the screen verbatim (§1.3 — unknown or
    // undecidable syntax degrades to visible raw text, never lost).
    let mut claimed: Vec<usize> = rows.iter().map(|r| r.line).collect();
    claimed.push(sep_line);
    claimed.sort_unstable();
    let claimed_total = claimed.len();
    claimed.dedup();
    if claimed.len() != claimed_total {
        return None;
    }

    let content_lines = per_line_content(content, idx, range, hint);

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
