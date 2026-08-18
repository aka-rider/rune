use std::collections::HashSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

use crate::width::display_width;

pub(crate) fn display_spans(
    display: &str,
    indices: &[u32],
    dim_style: Style,
    base_style: Style,
    avail_w: usize,
    dir_end: usize,
) -> Vec<Span<'static>> {
    let graphemes: Vec<(usize, &str)> = display.grapheme_indices(true).collect();
    let matched_graphemes = grapheme_match_mask(display, &graphemes, indices);
    let total_w = display_width(display);
    let (start, truncated) = fit_suffix(&graphemes, total_w, avail_w);

    let mut spans = Vec::with_capacity(graphemes.len() - start + 1);
    if truncated {
        spans.push(Span::styled("\u{2026}".to_string(), dim_style));
    }
    for (grapheme_idx, (byte_off, g)) in graphemes.iter().enumerate().skip(start) {
        let base = if *byte_off < dir_end {
            dim_style
        } else {
            base_style
        };
        let style = if matched_graphemes
            .get(grapheme_idx)
            .copied()
            .unwrap_or(false)
        {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        };
        spans.push(Span::styled((*g).to_string(), style));
    }
    spans
}

pub(crate) fn grapheme_match_mask(
    display: &str,
    graphemes: &[(usize, &str)],
    indices: &[u32],
) -> Vec<bool> {
    let matched: HashSet<usize> = indices.iter().map(|&i| i as usize).collect();
    let ascii_reduced = display.is_ascii()
        || graphemes
            .iter()
            .all(|(_, g)| g.chars().next().is_some_and(|c| c.is_ascii()));
    if ascii_reduced {
        graphemes
            .iter()
            .map(|(byte_off, _)| matched.contains(byte_off))
            .collect()
    } else {
        (0..graphemes.len()).map(|i| matched.contains(&i)).collect()
    }
}

pub(crate) fn fit_suffix(
    graphemes: &[(usize, &str)],
    total_w: usize,
    avail_w: usize,
) -> (usize, bool) {
    if total_w <= avail_w {
        return (0, false);
    }
    let ellipsis_w = display_width("\u{2026}");
    let budget = avail_w.saturating_sub(ellipsis_w);
    let mut used = 0usize;
    let mut start = graphemes.len();
    for (i, (_, g)) in graphemes.iter().enumerate().rev() {
        let w = display_width(g);
        if used + w > budget {
            break;
        }
        used += w;
        start = i;
    }
    (start, true)
}

pub(crate) fn with_bg(style: Style, bg: Option<Color>) -> Style {
    bg.map_or(style, |color| style.bg(color))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn nfd_decomposed_filename_bolds_the_matching_ascii_tail_graphemes() {
        let nfd_name = "cafe\u{0301}.md";
        let name_style = Style::new();
        let indices = vec![7, 8];
        let spans = display_spans(nfd_name, &indices, name_style, name_style, 80, 0);
        let bold_graphemes: Vec<String> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(bold_graphemes, vec!["m".to_string(), "d".to_string()]);

        let non_bold_graphemes: Vec<String> = spans
            .iter()
            .filter(|s| !s.style.add_modifier.contains(Modifier::BOLD))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(
            non_bold_graphemes,
            vec![
                "c".to_string(),
                "a".to_string(),
                "f".to_string(),
                "e\u{0301}".to_string(),
                ".".to_string(),
            ]
        );
    }
}
