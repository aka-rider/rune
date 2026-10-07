use std::ops::Range;

use super::prefix::is_blank;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnterPlan {
    Plain,
    Insert {
        replace: Range<usize>,
        continuation: String,
    },
    Replace {
        range: Range<usize>,
        text: String,
    },
    ExitQuote {
        previous: Range<usize>,
        current: Range<usize>,
    },
}

pub(super) fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

pub(super) fn locate(line: &str, caret: usize) -> Option<(&str, usize)> {
    let line = strip_cr(line);
    let caret = caret.min(line.len());
    line.is_char_boundary(caret).then_some((line, caret))
}

pub(super) fn insert_plan(line: &str, caret: usize, floor: usize, continuation: &str) -> EnterPlan {
    let before = line.get(..caret).unwrap_or("");
    let tail = line.get(caret..).unwrap_or("");
    let from = before
        .trim_end_matches(is_blank)
        .len()
        .max(floor)
        .min(caret);
    let to = line.len() - tail.trim_start_matches(is_blank).len();
    EnterPlan::Insert {
        replace: from..to,
        continuation: continuation.to_string(),
    }
}
