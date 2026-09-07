use std::ops::Range;

use rune_core::buffer::clamp_to_char_boundary;

use crate::commands::nav::word_range_at;
use crate::document::Document;
use crate::theme::Theme;

use super::{Cell, paint_range};

const MAX_NEEDLE_BYTES: usize = 200;

pub(super) fn apply_selection_matches(rows: &mut [Vec<Cell>], doc: &Document, theme: &Theme) {
    if !doc.shows_selection() {
        return;
    }
    let primary = doc.cursors.primary();
    if !primary.has_selection() {
        return;
    }
    let (start, end) = primary.selection_range();
    let (start, end) = (start.get(), end.get());
    let Some(needle) = doc.buffer.slice(start, end).and_then(usable_needle) else {
        return;
    };
    let Some(window) = super::overlay::visible_byte_range(rows) else {
        return;
    };

    let content = doc.buffer.content();
    let reach = needle.len().saturating_sub(1);
    let lo = clamp_to_char_boundary(content, window.start.saturating_sub(reach));
    let hi = content.ceil_char_boundary(window.end.saturating_add(reach));

    let needle_is_word = word_range_at(&doc.buffer, start) == (start, end);
    let selections: Vec<(usize, usize)> = doc
        .cursors
        .all()
        .iter()
        .filter(|cursor| cursor.has_selection())
        .map(|cursor| {
            let (s, e) = cursor.selection_range();
            (s.get(), e.get())
        })
        .collect();

    for hit in hits_in_window(content, needle, lo..hi) {
        if selections.contains(&(hit.start, hit.end)) {
            continue;
        }
        if needle_is_word && word_range_at(&doc.buffer, hit.start) != (hit.start, hit.end) {
            continue;
        }
        paint_range(rows, hit, theme.chrome.selection_match_bg);
    }
}

fn usable_needle(text: &str) -> Option<&str> {
    let rejected = text.trim().is_empty()
        || text.len() > MAX_NEEDLE_BYTES
        || text.contains('\n')
        || text.contains('\r');
    if rejected { None } else { Some(text) }
}

fn hits_in_window<'a>(
    content: &'a str,
    needle: &'a str,
    window: Range<usize>,
) -> impl Iterator<Item = Range<usize>> + 'a {
    let start = window.start;
    content
        .get(window)
        .unwrap_or_default()
        .match_indices(needle)
        .map(move |(at, hit)| start + at..start + at + hit.len())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "selection_match_tests.rs"]
mod tests;
