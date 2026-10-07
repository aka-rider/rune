use super::plan::{EnterPlan, insert_plan, locate};
use super::prefix::{Prefix, is_blank, parse_prefix};

pub fn enter_list(
    line: &str,
    caret: usize,
    prev_line: Option<&str>,
    parent: Option<&Prefix>,
) -> EnterPlan {
    let Some((line, caret)) = locate(line, caret) else {
        return EnterPlan::Plain;
    };
    let prefix = parse_prefix(line);
    if prefix.marker.is_none() && prefix.quote.depth == 0 {
        return EnterPlan::Plain;
    }
    if caret < prefix.content_start || opens_fence(line, &prefix) {
        return EnterPlan::Plain;
    }
    if prefix.empty {
        return enter_empty(line, &prefix, prev_line, parent);
    }
    match prefix.continuation() {
        Some(next) => insert_plan(line, caret, prefix.content_start, &next),
        None => EnterPlan::Plain,
    }
}

fn opens_fence(line: &str, prefix: &Prefix) -> bool {
    line.get(prefix.content_start..)
        .is_some_and(|rest| rest.starts_with("```") || rest.starts_with("~~~"))
}

fn enter_empty(
    line: &str,
    prefix: &Prefix,
    prev_line: Option<&str>,
    parent: Option<&Prefix>,
) -> EnterPlan {
    if prefix.marker.is_some() {
        return leave_item(line, prefix, parent);
    }
    let previous = prev_line.map(parse_prefix).filter(|prev| {
        prev.marker.is_none() && prev.empty && prev.quote.depth == prefix.quote.depth
    });
    match previous {
        Some(prev) => EnterPlan::ExitQuote {
            previous: prev.quote.span.start..prev.content_start,
            current: prefix.quote.span.start..prefix.content_start,
        },
        None => EnterPlan::Insert {
            replace: line.trim_end_matches(is_blank).len()..line.len(),
            continuation: prefix.lead.clone(),
        },
    }
}

fn leave_item(line: &str, prefix: &Prefix, parent: Option<&Prefix>) -> EnterPlan {
    let sibling_of_parent = parent
        .filter(|outer| outer.encloses(prefix))
        .and_then(Prefix::continuation);
    if let Some(text) = sibling_of_parent {
        return EnterPlan::Replace {
            range: 0..line.len(),
            text,
        };
    }
    let from = if prefix.quote.depth == 0 {
        0
    } else {
        prefix.quote.span.end
    };
    EnterPlan::Replace {
        range: from..line.len(),
        text: String::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "enter_tests.rs"]
mod tests;
