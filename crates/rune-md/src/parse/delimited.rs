//! The one breakdown of a block whose body lines sit between an opening and
//! a closing delimiter line — a fenced code block, or frontmatter.

use super::{ScanHint, last_line_of, line_at, line_end_at};
use rune_syntax::element::ByteRange;

/// `close` is absent when the block was never terminated, in which case its
/// last line is body.
pub(crate) struct Lines {
    pub(crate) open: ByteRange,
    pub(crate) close: Option<ByteRange>,
    pub(crate) content_lines: Vec<ByteRange>,
}

/// Break `range` into its opening delimiter line, its body lines, and — when
/// `terminated` — its closing delimiter line, all derived from `starts` so a
/// block's own internal line structure comes from the one line index every
/// consumer of `line_at` shares.
///
/// `terminated` is the caller's to establish and can never be inferred here:
/// a block spanning several lines is just as likely still being typed, with
/// no closing delimiter yet and a last line that is live body.
///
/// `open` starts at `range.start`, NOT at a `hint`-derived start — a node's
/// own sourcepos already bakes in every ancestor's first-line prefix (a
/// blockquote's `"> "` AND a list item's `"- "`/`"1. "`), whereas `hint`
/// only tracks REPEATING blockquote markers and would fall back to the
/// physical line start, re-claiming bytes a list item's own marker already
/// hid. Every later line IS a continuation line, which is exactly what
/// `hint` handles, and each gets its own range — never one contiguous span —
/// because a single range cannot exclude an interior container prefix.
pub(crate) fn split(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    hint: &ScanHint,
    terminated: bool,
) -> Lines {
    let len = content.len();
    let first_line = line_at(starts, range.start);
    let last_line = last_line_of(starts, range);
    let line_range = |line: usize, start: usize| {
        ByteRange::new(start, line_end_at(len, starts, line)).clamp(len)
    };

    let open = line_range(first_line, range.start);
    let (close, body) = if terminated {
        let close = line_range(last_line, hint.start_for_line(starts, last_line));
        (Some(close), (first_line + 1)..last_line)
    } else {
        (None, (first_line + 1)..(last_line + 1))
    };
    let content_lines = body
        .map(|line| line_range(line, hint.start_for_line(starts, line)))
        .collect();

    Lines {
        open,
        close,
        content_lines,
    }
}
