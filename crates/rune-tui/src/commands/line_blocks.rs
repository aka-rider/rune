use std::ops::Range;

use rune_core::buffer::Buffer;
use rune_core::cursor::Cursor;

pub(crate) struct LineBlock {
    pub lines: Range<usize>,
    pub cursors: Vec<Cursor>,
}

pub(crate) fn selected_lines(c: &Cursor, buf: &Buffer) -> Range<usize> {
    let first = buf.offset_to_line_col(c.selection_start().get()).line;
    let mut last = buf.offset_to_line_col(c.selection_end().get()).line;
    if last > first && buf.line_start(last) == Some(c.selection_end().get()) {
        last -= 1;
    }
    first..last + 1
}

pub(crate) fn overlapping_line_blocks(cursors: &[Cursor], buf: &Buffer) -> Vec<LineBlock> {
    line_blocks(cursors, buf, |next_start, block_end| next_start < block_end)
}

pub(crate) fn touching_line_blocks(cursors: &[Cursor], buf: &Buffer) -> Vec<LineBlock> {
    line_blocks(cursors, buf, |next_start, block_end| {
        next_start <= block_end
    })
}

fn line_blocks(
    cursors: &[Cursor],
    buf: &Buffer,
    merges: impl Fn(usize, usize) -> bool,
) -> Vec<LineBlock> {
    let mut spans: Vec<(Range<usize>, Cursor)> = cursors
        .iter()
        .map(|c| (selected_lines(c, buf), *c))
        .collect();
    spans.sort_by_key(|(lines, _)| (lines.start, lines.end));
    let mut blocks: Vec<LineBlock> = Vec::new();
    for (lines, cursor) in spans {
        match blocks.last_mut() {
            Some(block) if merges(lines.start, block.lines.end) => {
                block.lines.end = block.lines.end.max(lines.end);
                block.cursors.push(cursor);
            }
            _ => blocks.push(LineBlock {
                lines,
                cursors: vec![cursor],
            }),
        }
    }
    blocks
}

#[cfg(test)]
#[path = "line_blocks_tests.rs"]
mod tests;
