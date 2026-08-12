//! Fixed-width continuation-indent derivation, shared by a list item's own
//! marker width and an indented code block's own leading whitespace — the
//! non-repeating counterpart to `blockquote_markers`'s per-line rescan.

use super::{ScanHint, last_line_of, line_at, line_end_at};
use rune_syntax::element::ByteRange;
use std::collections::HashMap;

/// One scan-start entry per line that carries `width` bytes of plain-space
/// indentation on top of `hint`'s own baseline — a line that falls short (a
/// lazily continued paragraph inside a list item, say) gets no entry, and
/// `ScanHint::start_for_line` falls back to `hint` itself: the same
/// fallback `blockquote_markers` relies on for its own lazy continuation
/// lines. Unlike a blockquote's `"> "`, this width is fixed once from the
/// owning block's own first line and never rescanned per line.
pub(super) fn fixed_indent_ends(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    width: usize,
    hint: &ScanHint,
) -> HashMap<usize, usize> {
    let first_line = line_at(starts, range.start);
    let last_line = last_line_of(starts, range);
    let mut ends = HashMap::new();
    for line in first_line..=last_line {
        let line_end = line_end_at(content.len(), starts, line);
        let scan_start = hint.start_for_line(starts, line);
        let candidate_end = scan_start.saturating_add(width).min(line_end);
        if candidate_end - scan_start != width {
            continue;
        }
        let Some(prefix) = content.get(scan_start..candidate_end) else {
            continue;
        };
        if prefix.bytes().all(|b| b == b' ') {
            ends.insert(line, candidate_end);
        }
    }
    ends
}
