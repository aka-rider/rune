use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

// Shared by the title bar and the find panel's fields: both paint a
// selection background plus a `Modifier::REVERSED` caret cell over a plain
// text line, differing only in what style a given byte offset gets outside
// selection/caret (`style_at`) — the title dims a locked extension, a
// query row does not. `style_bounds` names every offset where `style_at`
// can change independent of the selection/caret — the title's extension
// split — so a span never straddles a style change even where there is no
// selection to force the break.
pub(crate) fn caret_spans(
    text: &str,
    style_bounds: &[usize],
    selection: Option<(usize, usize, usize)>,
    selection_bg: Color,
    style_at: impl Fn(usize) -> Style,
) -> Vec<Span<'static>> {
    let len = text.len();
    let mut bounds = vec![0usize, len];
    for &bound in style_bounds {
        bounds.push(text.floor_char_boundary(bound.min(len)));
    }
    if let Some((start, end, cursor)) = selection {
        bounds.push(text.floor_char_boundary(start));
        bounds.push(text.floor_char_boundary(end));
        bounds.push(text.floor_char_boundary(cursor));
        bounds.push(text.floor_char_boundary(next_grapheme_end(text, cursor)));
    }
    bounds.sort_unstable();
    bounds.dedup();

    let mut spans = Vec::new();
    for pair in bounds.windows(2) {
        let &[a, b] = pair else { continue };
        let seg = text.get(a..b).unwrap_or("");
        if seg.is_empty() {
            continue;
        }
        let mut style = style_at(a);
        if let Some((start, end, cursor)) = selection {
            if start != end && a >= start && b <= end {
                style = style.bg(selection_bg);
            }
            if a == cursor {
                style = style.add_modifier(Modifier::REVERSED);
            }
        }
        spans.push(Span::styled(seg.to_string(), style));
    }

    if let Some((_, _, cursor)) = selection
        && cursor == len
    {
        let style = style_at(cursor).add_modifier(Modifier::REVERSED);
        spans.push(Span::styled(" ", style));
    }

    spans
}

fn next_grapheme_end(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text.get(at..)
        .and_then(|rest| rest.graphemes(true).next())
        .map_or(text.len(), |g| at + g.len())
}
