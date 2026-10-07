use std::ops::Range;

use super::comment::find_comment;
use super::plan::locate;
use super::prefix::parse_prefix;

pub fn backspace(line: &str, caret: usize, tokens: Option<&[&str]>) -> Option<Range<usize>> {
    let (line, caret) = locate(line, caret)?;
    if caret != line.len() {
        return None;
    }
    match tokens {
        Some(tokens) => {
            let comment = find_comment(line, tokens)?;
            comment.empty.then_some(comment.token_start..line.len())
        }
        None => {
            let prefix = parse_prefix(line);
            if !prefix.empty {
                return None;
            }
            match &prefix.marker {
                Some(marker) => Some(marker.span.start..line.len()),
                None => (prefix.quote.depth > 0).then_some(prefix.quote.last.start..line.len()),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "backspace_tests.rs"]
mod tests;
