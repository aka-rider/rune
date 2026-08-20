use super::{ScanHint, last_line_of, line_at, line_end_at};
use rune_syntax::element::ByteRange;

pub(crate) struct Lines {
    pub(crate) open: ByteRange,
    pub(crate) close: Option<ByteRange>,
    pub(crate) content_lines: Vec<ByteRange>,
}

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
