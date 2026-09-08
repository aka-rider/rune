use std::ops::Range;

use ratatui::style::Style;
use rune_core::assert_invariant;
use rune_md::element::doc::ViewSnapshots;
use unicode_segmentation::UnicodeSegmentation;

use crate::app::App;
use crate::document::Document;

use super::{Cell, push_grapheme_cells};

pub(super) fn apply(rows: &mut [Vec<Cell>], app: &App, doc: &Document, view: &ViewSnapshots) {
    let Some(state) = app.find().filter(|state| state.focused) else {
        return;
    };
    let Some(replace) = state.replace.as_ref() else {
        return;
    };
    let Ok(matcher) = &state.pattern else {
        return;
    };
    if state.doc != app.active || state.buffer_version != doc.buffer.version() {
        return;
    }
    let Some(window) = super::overlay::visible_byte_range(rows) else {
        return;
    };
    let content = doc.buffer.content();
    let boxed: Vec<bool> = crate::row_meta::row_meta(view, app)
        .iter()
        .map(|meta| meta.boxed)
        .collect();
    let bounds = row_offset_bounds(rows);
    let current = state.current.and_then(|idx| state.matches.get(idx));
    for range in state
        .matches
        .iter()
        .filter(|m| m.start < window.end && m.end > window.start)
    {
        let Some(source) = content.get(range.clone()) else {
            continue;
        };
        let Some((row_idx, span)) = locate(rows, &bounds, range, source) else {
            continue;
        };
        if boxed.get(row_idx).copied().unwrap_or(false) {
            continue;
        }
        let Some(replacement) = matcher.replacement_at(content, range, &replace.draft) else {
            continue;
        };
        let tint = if current == Some(range) {
            app.theme.chrome.replace_preview_current_bg
        } else {
            app.theme.chrome.replace_preview_bg
        };
        if let Some(row) = rows.get_mut(row_idx) {
            splice(row, span, range.start, &replacement, tint);
        }
    }
}

fn row_offset_bounds(rows: &[Vec<Cell>]) -> Vec<Option<Range<usize>>> {
    rows.iter()
        .map(|row| {
            let mut bounds: Option<Range<usize>> = None;
            for offset in row.iter().filter_map(|cell| cell.buf_offset) {
                let offset = offset as usize;
                bounds = Some(match bounds {
                    None => offset..offset + 1,
                    Some(b) => b.start.min(offset)..b.end.max(offset + 1),
                });
            }
            bounds
        })
        .collect()
}

fn locate(
    rows: &[Vec<Cell>],
    bounds: &[Option<Range<usize>>],
    range: &Range<usize>,
    source: &str,
) -> Option<(usize, Range<usize>)> {
    let mut found: Option<(usize, Range<usize>)> = None;
    for (row_idx, row) in rows.iter().enumerate() {
        let overlaps = bounds
            .get(row_idx)
            .and_then(Option::as_ref)
            .is_some_and(|b| b.start < range.end && b.end > range.start);
        if !overlaps {
            continue;
        }
        let span = contiguous_span(row, range)?;
        if found.is_some() {
            return None;
        }
        found = Some((row_idx, span));
    }
    let (row_idx, span) = found?;
    let text: String = rows
        .get(row_idx)?
        .get(span.clone())?
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    (text == source).then_some((row_idx, span))
}

fn contiguous_span(row: &[Cell], range: &Range<usize>) -> Option<Range<usize>> {
    let mut span: Option<Range<usize>> = None;
    for (cell_idx, cell) in row.iter().enumerate() {
        let inside = cell
            .buf_offset
            .is_some_and(|offset| range.contains(&(offset as usize)));
        if !inside {
            continue;
        }
        span = Some(match span {
            None => cell_idx..cell_idx + 1,
            Some(span) if span.end == cell_idx => span.start..cell_idx + 1,
            Some(_) => return None,
        });
    }
    span
}

fn splice(row: &mut Vec<Cell>, span: Range<usize>, start: usize, replacement: &str, tint: Style) {
    let offset = u32::try_from(start).ok();
    assert_invariant!(offset.is_some(), || format!(
        "preview byte offset {start} exceeds the cell offset range"
    ));
    let base = row
        .get(span.start)
        .map_or_else(Style::default, |cell| cell.style);
    let style = base.patch(tint);
    let cells = if replacement.is_empty() {
        let width: usize = row.get(span.clone()).map_or(0, |cells| {
            cells.iter().map(|cell| usize::from(cell.width)).sum()
        });
        vec![
            Cell {
                text: " ".into(),
                width: 1,
                style,
                buf_offset: offset,
            };
            width
        ]
    } else {
        let mut visual_col: usize = row.get(..span.start).map_or(0, |cells| {
            cells.iter().map(|cell| usize::from(cell.width)).sum()
        });
        let mut cells = Vec::new();
        for grapheme in replacement.graphemes(true) {
            push_grapheme_cells(&mut cells, &mut visual_col, grapheme, offset, style);
        }
        cells
    };
    row.splice(span, cells);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "replace_preview_tests.rs"]
mod tests;
