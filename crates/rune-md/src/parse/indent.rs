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
        if candidate_end.saturating_sub(scan_start) != width {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rune_syntax::element::ByteRange;

    /// A hint whose `start_for_line` claims past a line's own end (`scan_start
    /// > line_end`, the same overshooting shape `per_line_content`'s own
    /// `per_line_content_clamps_a_hint_start_that_overshoots_its_own_line`
    /// test pins against a sibling function) must skip that line rather than
    /// underflow `candidate_end - scan_start`.
    #[test]
    fn a_hint_start_past_the_lines_end_is_skipped_not_underflowed() {
        let content = "x".repeat(15);
        let starts = vec![0, 5, 10];
        let range = ByteRange::new(0, 15);
        let hint = ScanHint::Nested {
            marker_ends: std::collections::HashMap::from([(1, 12)]),
            conceals_own_prefix: true,
            parent: &ScanHint::Root,
        };

        let ends = fixed_indent_ends(&content, &starts, range, 3, &hint);

        assert!(!ends.contains_key(&1));
    }
}
