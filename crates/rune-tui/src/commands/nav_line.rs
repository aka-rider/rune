//! Line/document-oriented motion commands: split out of the sibling `nav`
//! module (`nav` was already over the 500-line budget). Character and
//! word motion, the word/whitespace classifier, and the shared
//! cursor-stepping infrastructure (`move_cursors`, `update_horizontal`) all
//! stay in `nav`; this module reaches back into `nav` (via `pub(crate)`)
//! for that shared infrastructure rather than duplicating it.

use rune_core::buffer::Buffer;
use rune_core::coords::BufferPoint;
use rune_core::cursor::Cursor;
use rune_md::element::doc::ViewSnapshots;

use crate::commands::nav::{move_cursors, update_horizontal};
use crate::document::Document;
use crate::keymap::Extend;

/// The "smart home" offset for the line containing `offset`: toggles
/// between the line's first non-whitespace column and column 0.
pub fn line_start_offset(buf: &Buffer, offset: usize) -> usize {
    let bp = buf.offset_to_line_col(offset);
    let line_start = buf.line_col_to_offset(BufferPoint {
        line: bp.line,
        col: 0,
    });

    let mut first_non_ws = line_start;
    while first_non_ws < buf.len() {
        let Some((r, size)) = buf.rune_at(first_non_ws) else {
            break;
        };
        if r == '\n' || (r != ' ' && r != '\t') {
            break;
        }
        first_non_ws += size;
    }

    if offset == first_non_ws {
        line_start
    } else {
        first_non_ws
    }
}

pub fn line_end_offset(buf: &Buffer, offset: usize) -> usize {
    let bp = buf.offset_to_line_col(offset);
    buf.line_content_end(bp.line).unwrap_or(buf.len())
}

/// The byte range `[line_start, line_end)` of the line containing `offset`,
/// extended to include the line's trailing `\n` unless it's the buffer's
/// last line. The shared chokepoint for "whole current line" ranges, used
/// identically by `commands::clipboard::copy_entire_line` (what gets
/// copied) and `commands::edit::delete_selection_or_line` (what cut
/// removes) so the two can never disagree about where a line-copy ends.
pub(crate) fn line_range_incl_newline(buf: &Buffer, offset: usize) -> (usize, usize) {
    let bp = buf.offset_to_line_col(offset);
    let line_start = buf.line_start(bp.line).unwrap_or(0);
    let line_end = buf
        .line_terminator_range(bp.line)
        .map_or(buf.len(), |r| r.end);
    (line_start, line_end)
}

/// Resolves `c`'s new position via `step`, then routes it through
/// `update_horizontal` so the desired visual column and any active
/// selection are handled consistently with other motions.
fn handle_move_to(
    view: &ViewSnapshots,
    buf: &Buffer,
    c: Cursor,
    extend: Extend,
    step: impl Fn(&Buffer, usize) -> usize,
) -> Cursor {
    let offset = step(buf, c.position);
    update_horizontal(view, buf, c, offset, extend)
}

pub fn line_start(doc: &mut Document, extend: Extend) {
    move_cursors(doc, extend, |view, buf, c, extend| {
        handle_move_to(view, buf, c, extend, line_start_offset)
    });
}

pub fn line_end(doc: &mut Document, extend: Extend) {
    move_cursors(doc, extend, |view, buf, c, extend| {
        handle_move_to(view, buf, c, extend, line_end_offset)
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn line_start_offset_toggles_first_non_ws_and_column_zero() {
        let buf = Buffer::new("   indented\n");
        // Cursor already at the first non-whitespace column: toggling goes
        // to column 0.
        assert_eq!(line_start_offset(&buf, 3), 0);
        // Cursor elsewhere on the line: goes to the first non-whitespace
        // column.
        assert_eq!(line_start_offset(&buf, 7), 3);
    }

    #[test]
    fn line_end_offset_stops_before_the_newline() {
        let buf = Buffer::new("hello\nworld\n");
        assert_eq!(line_end_offset(&buf, 0), 5);
        assert_eq!(line_end_offset(&buf, 3), 5);
    }

    #[test]
    fn end_key_lands_before_the_cr_of_a_crlf_line_not_between_cr_and_lf() {
        let buf = Buffer::new("abc\r\ndef\r\n");
        assert_eq!(line_end_offset(&buf, 0), 3);
        assert_eq!(line_end_offset(&buf, 5), 8);
    }

    #[test]
    fn line_range_incl_newline_spans_the_whole_crlf_terminator() {
        let buf = Buffer::new("abc\r\ndef");
        assert_eq!(line_range_incl_newline(&buf, 0), (0, 5));
        assert_eq!(line_range_incl_newline(&buf, 5), (5, 8));
    }
}
