use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::theme::Theme;
use crate::width::{display_width, truncate_to_width};

pub(crate) struct QueryRow<'a> {
    pub prompt: &'a str,
    pub draft: &'a str,
    pub readout: Option<(&'a str, Style)>,
    pub focused: bool,
}

pub(crate) fn build_spans(row: QueryRow<'_>, area_w: usize, theme: &Theme) -> Vec<Span<'static>> {
    let prompt_w = display_width(row.prompt).min(area_w);
    let prompt_shown = truncate_to_width(row.prompt, prompt_w);
    let readout_w = row.readout.map_or(0, |(text, _)| display_width(text));
    let caret_w = usize::from(row.focused);
    let gap = usize::from(readout_w > 0 && area_w > prompt_w + readout_w);
    let draft_budget = area_w
        .saturating_sub(prompt_w)
        .saturating_sub(caret_w)
        .saturating_sub(readout_w + gap);
    let draft_shown = truncate_to_width(row.draft, draft_budget);
    let draft_w = display_width(&draft_shown);
    let pad = area_w.saturating_sub(prompt_w + draft_w + caret_w + readout_w);

    let mut spans = Vec::new();
    if !prompt_shown.is_empty() {
        spans.push(Span::styled(prompt_shown, theme.chrome.active_border));
    }
    spans.push(Span::styled(draft_shown, theme.chrome.title_text));
    if row.focused {
        spans.push(Span::styled(
            " ",
            theme.chrome.title_text.add_modifier(Modifier::REVERSED),
        ));
    }
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    if let Some((readout, style)) = row.readout {
        spans.push(Span::styled(readout.to_string(), style));
    }
    spans
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const PROMPT: &str = "/ ";

    fn theme() -> Theme {
        Theme::catppuccin_mocha(false)
    }

    fn row<'a>(
        draft: &'a str,
        readout: Option<&'a str>,
        focused: bool,
        theme: &Theme,
    ) -> QueryRow<'a> {
        QueryRow {
            prompt: PROMPT,
            draft,
            readout: readout.map(|text| (text, theme.chrome.title_text)),
            focused,
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
                draft: "term",
                readout: None,
                focused: false,
            },
            40,
            &theme,
        );
        assert!(joined(&spans).starts_with("term"));
    }
}
