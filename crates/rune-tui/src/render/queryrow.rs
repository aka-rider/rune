use std::ops::Range;

use ratatui::style::Style;
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

use crate::render::caret::caret_spans;
use crate::theme::Theme;
use crate::width::{display_width, truncate_to_width};

pub(crate) struct QueryRow<'a> {
    pub prompt: &'a str,
    pub text: &'a str,
    pub readout: Option<(&'a str, Style)>,
    pub focused: bool,
    pub cursor: usize,
    pub selection: (usize, usize),
}

pub(crate) fn build_spans(row: QueryRow<'_>, area_w: usize, theme: &Theme) -> Vec<Span<'static>> {
    let prompt_w = display_width(row.prompt).min(area_w);
    let prompt_shown = truncate_to_width(row.prompt, prompt_w);
    let readout_w = row.readout.map_or(0, |(text, _)| display_width(text));
    let gap = usize::from(readout_w > 0 && area_w > prompt_w + readout_w);
    let draft_budget = area_w
        .saturating_sub(prompt_w)
        .saturating_sub(readout_w + gap);

    let (draft_shown, local_cursor, local_selection) = if row.focused {
        let caret_reserve = usize::from(row.cursor >= row.text.len());
        let text_budget = draft_budget.saturating_sub(caret_reserve);
        windowed(row.text, row.cursor, row.selection, text_budget)
    } else {
        (truncate_to_width(row.text, draft_budget), 0, (0, 0))
    };

    let caret_cell = usize::from(row.focused && local_cursor >= draft_shown.len());
    let drawn_w = display_width(&draft_shown) + caret_cell;

    let mut spans = Vec::new();
    if !prompt_shown.is_empty() {
        spans.push(Span::styled(prompt_shown, theme.chrome.active_border));
    }
    if row.focused {
        spans.extend(caret_spans(
            &draft_shown,
            &[],
            Some((local_selection.0, local_selection.1, local_cursor)),
            theme.chrome.selection_bg,
            |_| theme.chrome.title_text,
        ));
    } else {
        spans.push(Span::styled(draft_shown, theme.chrome.title_text));
    }

    let pad = area_w.saturating_sub(prompt_w + drawn_w + readout_w);
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    if let Some((readout, style)) = row.readout {
        spans.push(Span::styled(readout.to_string(), style));
    }
    spans
}

// Slices `text` down to whatever fits in `budget` cells while keeping
// `cursor` inside the slice — growing right from the caret first (so text
// still ahead of it stays visible), then left with whatever budget is
// left, so a query wider than the field always trims from the left as the
// caret sits at or near the end.
fn windowed(
    text: &str,
    cursor: usize,
    selection: (usize, usize),
    budget: usize,
) -> (String, usize, (usize, usize)) {
    if display_width(text) <= budget {
        return (text.to_string(), cursor, selection);
    }
    let window = window_for_caret(text, cursor, budget);
    let shown = text.get(window.clone()).unwrap_or("").to_string();
    let local_cursor = cursor.clamp(window.start, window.end) - window.start;
    let local_selection = (
        selection.0.clamp(window.start, window.end) - window.start,
        selection.1.clamp(window.start, window.end) - window.start,
    );
    (shown, local_cursor, local_selection)
}

fn window_for_caret(text: &str, cursor: usize, budget: usize) -> Range<usize> {
    let cursor = text.floor_char_boundary(cursor.min(text.len()));
    let mut end = cursor;
    let mut used = 0usize;
    for g in text.get(cursor..).unwrap_or("").graphemes(true) {
        let w = display_width(g);
        if used + w > budget {
            break;
        }
        used += w;
        end += g.len();
    }
    let mut start = cursor;
    for g in text.get(..cursor).unwrap_or("").graphemes(true).rev() {
        let w = display_width(g);
        if used + w > budget {
            break;
        }
        used += w;
        start -= g.len();
    }
    start..end
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use ratatui::style::Modifier;

    use super::*;

    const PROMPT: &str = "/ ";

    fn theme() -> Theme {
        Theme::catppuccin_mocha(false)
    }

    fn row<'a>(
        text: &'a str,
        readout: Option<&'a str>,
        focused: bool,
        theme: &Theme,
    ) -> QueryRow<'a> {
        let cursor = text.len();
        QueryRow {
            prompt: PROMPT,
            text,
            readout: readout.map(|text| (text, theme.chrome.title_text)),
            focused,
            cursor,
            selection: (cursor, cursor),
        }
    }

    fn joined(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn focused_row_paints_a_leading_prompt_and_a_trailing_caret() {
        let theme = theme();
        let spans = build_spans(row("hi", None, true, &theme), 40, &theme);
        let prompt = spans.first().expect("a prompt span");
        assert_eq!(prompt.content, PROMPT);
        assert_eq!(prompt.style, theme.chrome.active_border);

        let caret = spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("a reversed caret span");
        assert_eq!(caret.content, " ");
    }

    #[test]
    fn an_unfocused_row_paints_no_caret() {
        let theme = theme();
        let spans = build_spans(row("hi", None, false, &theme), 40, &theme);
        assert!(
            spans
                .iter()
                .all(|s| !s.style.add_modifier.contains(Modifier::REVERSED))
        );
    }

    #[test]
    fn the_full_row_reconstructs_prompt_draft_and_readout() {
        let theme = theme();
        let spans = build_spans(row("term", Some("2/3"), true, &theme), 40, &theme);
        let text = joined(&spans);
        assert!(text.starts_with(PROMPT));
        assert!(text.contains("term"));
        assert!(text.trim_end().ends_with("2/3"));
    }

    #[test]
    fn an_empty_prompt_starts_the_row_with_the_draft() {
        let theme = theme();
        let spans = build_spans(
            QueryRow {
                prompt: "",
                text: "term",
                readout: None,
                focused: false,
                cursor: 4,
                selection: (4, 4),
            },
            40,
            &theme,
        );
        assert!(joined(&spans).starts_with("term"));
    }

    #[test]
    fn a_mid_text_caret_reverses_the_character_under_it_not_a_trailing_space() {
        let theme = theme();
        let spans = build_spans(
            QueryRow {
                prompt: "",
                text: "hello",
                readout: None,
                focused: true,
                cursor: 2,
                selection: (2, 2),
            },
            40,
            &theme,
        );
        let caret = spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("a reversed caret span");
        assert_eq!(caret.content, "l");
    }

    #[test]
    fn a_selection_gets_the_selection_background() {
        let theme = theme();
        let spans = build_spans(
            QueryRow {
                prompt: "",
                text: "hello",
                readout: None,
                focused: true,
                cursor: 3,
                selection: (0, 3),
            },
            40,
            &theme,
        );
        let selected = spans.iter().find(|s| s.content == "hel").unwrap();
        assert_eq!(selected.style.bg, Some(theme.chrome.selection_bg));
    }

    #[test]
    fn a_query_wider_than_the_field_still_ends_with_the_caret_cell() {
        let theme = theme();
        let long = "a".repeat(30);
        let spans = build_spans(
            QueryRow {
                prompt: "",
                text: &long,
                readout: None,
                focused: true,
                cursor: long.len(),
                selection: (long.len(), long.len()),
            },
            10,
            &theme,
        );
        let last = spans.last().expect("at least the caret span");
        assert!(last.style.add_modifier.contains(Modifier::REVERSED));
        let total_width: usize = spans.iter().map(|s| display_width(&s.content)).sum();
        assert!(total_width <= 10, "row must not overflow the field width");
    }
}
