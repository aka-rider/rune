use super::plan::{EnterPlan, insert_plan, locate};
use super::prefix::is_blank;

pub(super) struct CommentLine<'a> {
    pub indent: &'a str,
    pub token: &'a str,
    pub token_start: usize,
    pub token_end: usize,
    pub empty: bool,
}

pub(super) fn find_comment<'a>(line: &'a str, tokens: &[&'a str]) -> Option<CommentLine<'a>> {
    let rest = line.trim_start_matches(is_blank);
    let token_start = line.len() - rest.len();
    let token = tokens
        .iter()
        .copied()
        .filter(|t| !t.is_empty() && rest.starts_with(t))
        .max_by_key(|t| t.len())?;
    let token_end = token_start + token.len();
    Some(CommentLine {
        indent: line.get(..token_start)?,
        token,
        token_start,
        token_end,
        empty: line.get(token_end..)?.chars().all(is_blank),
    })
}

pub fn enter_comment(line: &str, caret: usize, tokens: &[&str], is_first_line: bool) -> EnterPlan {
    let Some((line, caret)) = locate(line, caret) else {
        return EnterPlan::Plain;
    };
    if is_first_line && line.starts_with("#!") {
        return EnterPlan::Plain;
    }
    let Some(comment) = find_comment(line, tokens) else {
        return EnterPlan::Plain;
    };
    if caret < comment.token_end {
        return EnterPlan::Plain;
    }
    if comment.empty {
        return EnterPlan::Replace {
            range: comment.token_start..line.len(),
            text: String::new(),
        };
    }
    let continuation = format!("{}{} ", comment.indent, comment.token);
    insert_plan(line, caret, comment.token_end, &continuation)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "comment_tests.rs"]
mod tests;
