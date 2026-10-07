use super::prefix::{Prefix, parse_prefix};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parent {
    pub lines_above: usize,
    pub prefix: Prefix,
}

pub const PARENT_SCAN_LINES: usize = 200;

pub fn find_parent<'a>(child: &Prefix, above: impl IntoIterator<Item = &'a str>) -> Option<Parent> {
    let column = child.marker_column()?;
    for (lines_above, line) in above.into_iter().take(PARENT_SCAN_LINES).enumerate() {
        let candidate = parse_prefix(line);
        if candidate.quote.depth != child.quote.depth {
            return None;
        }
        match candidate.marker_column() {
            Some(outer) if outer < column => {
                return Some(Parent {
                    lines_above,
                    prefix: candidate,
                });
            }
            Some(_) => {}
            None if candidate.empty || candidate.content_column() >= column => {}
            None => return None,
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "parent_tests.rs"]
mod tests;
