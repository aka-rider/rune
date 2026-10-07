use std::ops::Range;

use super::prefix::{MAX_ORDERED, MarkerKind, Prefix, format_number, parse_prefix};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberEdit {
    pub line: usize,
    pub range: Range<usize>,
    pub text: String,
    from: u32,
    shift: u32,
    width: usize,
}

impl NumberEdit {
    pub fn shift(&self) -> u32 {
        self.shift
    }

    pub fn absorb(&mut self, other: &NumberEdit) {
        self.shift = self.shift.saturating_add(other.shift);
        let number = self.from.saturating_add(self.shift).min(MAX_ORDERED);
        self.text = format_number(number, self.width);
    }
}

pub fn renumber<'a>(
    of: &Prefix,
    following: impl IntoIterator<Item = &'a str>,
    inserted: u32,
) -> Vec<NumberEdit> {
    let mut edits = Vec::new();
    let (Some(item), true) = (&of.marker, inserted > 0) else {
        return edits;
    };
    let MarkerKind::Ordered { n, delim, .. } = item.kind else {
        return edits;
    };
    let mut expected = n + 1;
    for (line, text) in following.into_iter().enumerate() {
        let sibling = parse_prefix(text);
        if sibling.quote.depth != of.quote.depth {
            break;
        }
        let Some(marker) = &sibling.marker else {
            if sibling.empty || sibling.content_column() > item.column {
                continue;
            }
            break;
        };
        if marker.column > item.column {
            continue;
        }
        let MarkerKind::Ordered {
            n: found,
            delim: found_delim,
            width,
        } = marker.kind
        else {
            break;
        };
        if marker.column < item.column || found_delim != delim || found != expected {
            break;
        }
        let Some(shifted) = found.checked_add(inserted).filter(|v| *v <= MAX_ORDERED) else {
            break;
        };
        edits.push(NumberEdit {
            line,
            range: marker.symbol.start..marker.symbol.start + width,
            text: format_number(shifted, width),
            from: found,
            shift: inserted,
            width,
        });
        expected = found + 1;
    }
    edits
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "renumber_tests.rs"]
mod tests;
