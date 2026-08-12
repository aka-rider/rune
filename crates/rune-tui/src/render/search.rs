//! Renders the search bar's one row: a leading prompt glyph (the bar's own
//! focus affordance — ^F otherwise reads as the editor growing a blank row
//! while the editor's own caret keeps blinking underneath it), the live
//! draft with a trailing caret cell while focused, and a right-aligned
//! readout (`i/N`, `N matches`, or `no matches`). A sibling of `search`
//! (the state module), not a descendant — it reads `SearchState` only
//! through the `pub(crate)` fields that module marks for exactly this.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::theme::Theme;
use crate::width::{display_width, truncate_to_width};

/// The bar's own focus affordance, styled in `theme.chrome.active_border`
/// — the same "this region holds focus" accent the messages pane's
/// separator uses (`messages::render::draw`).
const PROMPT: &str = "/ ";

/// Pure function of `&App`: drawing twice produces identical output, the
/// same guarantee `render::title::draw` makes. A no-op if the bar isn't
/// open — `render::draw` already gates this call on `Geometry::search_bar`
/// being `Some`, which itself requires `App::search` to be `Some`, but the
/// guard stays here too rather than trusting that chain silently.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let Some(state) = app.search() else {
        return;
    };
    let readout = readout_text(
        state.matches.len(),
        state.current,
        state.draft.trim().is_empty(),
    );
    let spans = build_spans(
        &state.draft,
        readout.as_deref(),
        state.focused,
        area.width as usize,
        &app.theme,
    );
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Builds the styled spans for one query row: `PROMPT`, then as much of
/// `draft` as fits, a reversed-video caret cell while `focused`, padding,
/// and finally `readout` right-aligned. Pure so it's testable without a
/// `Frame` — the same split `render::title::build_spans` uses. `pub(crate)`:
/// the fuzzy file finder's own query row (`render::filesearch`) reuses this
/// exact chokepoint rather than forking the prompt/caret/readout logic.
pub(crate) fn build_spans(
    draft: &str,
    readout: Option<&str>,
    focused: bool,
    area_w: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let prompt_w = display_width(PROMPT).min(area_w);
    let prompt_shown = truncate_to_width(PROMPT, prompt_w);
    let readout_w = readout.map_or(0, display_width);
    let caret_w = usize::from(focused);
    let gap = usize::from(readout_w > 0 && area_w > prompt_w + readout_w);
    let draft_budget = area_w
        .saturating_sub(prompt_w)
        .saturating_sub(caret_w)
        .saturating_sub(readout_w + gap);
    let draft_shown = truncate_to_width(draft, draft_budget);
    let draft_w = display_width(&draft_shown);
    let pad = area_w.saturating_sub(prompt_w + draft_w + caret_w + readout_w);

    let mut spans = Vec::new();
    if !prompt_shown.is_empty() {
        spans.push(Span::styled(prompt_shown, theme.chrome.active_border));
    }
    spans.push(Span::styled(draft_shown, theme.chrome.title_text));
    if focused {
        spans.push(Span::styled(
            " ",
            theme.chrome.title_text.add_modifier(Modifier::REVERSED),
        ));
    }
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    if let Some(readout) = readout {
        spans.push(Span::styled(readout.to_string(), theme.chrome.title_text));
    }
    spans
}

/// The bar's right-aligned status text: `i/N` once a match is selected
/// (`current`), else `N matches` while any exist, else `no matches` for a
/// non-empty query with zero hits, else nothing at all for an empty draft
/// — matching an empty query never claims to have "no matches" for a
/// search that hasn't started yet.
fn readout_text(count: usize, current: Option<usize>, draft_empty: bool) -> Option<String> {
    if let Some(i) = current {
        Some(format!("{}/{count}", i + 1))
    } else if count > 0 {
        Some(format!("{count} matches"))
    } else if !draft_empty {
        Some("no matches".to_string())
    } else {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::catppuccin_mocha(false)
    }

    fn joined(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn focused_bar_paints_a_leading_prompt_and_a_trailing_caret() {
        let theme = theme();
        let spans = build_spans("hi", None, true, 40, &theme);
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
    fn an_unfocused_bar_paints_no_caret() {
        let theme = theme();
        let spans = build_spans("hi", None, false, 40, &theme);
        assert!(
            spans
                .iter()
                .all(|s| !s.style.add_modifier.contains(Modifier::REVERSED))
        );
    }

    #[test]
    fn the_full_row_reconstructs_prompt_draft_and_readout() {
        let theme = theme();
        let spans = build_spans("term", Some("2/3"), true, 40, &theme);
        let text = joined(&spans);
        assert!(text.starts_with(PROMPT));
        assert!(text.contains("term"));
        assert!(text.trim_end().ends_with("2/3"));
    }

    #[test]
    fn a_selected_match_reads_as_i_of_n() {
        assert_eq!(readout_text(3, Some(1), false), Some("2/3".to_string()));
    }

    #[test]
    fn matches_with_no_selection_read_as_a_count() {
        assert_eq!(readout_text(3, None, false), Some("3 matches".to_string()));
    }

    #[test]
    fn a_non_empty_query_with_no_hits_reads_as_no_matches() {
        assert_eq!(readout_text(0, None, false), Some("no matches".to_string()));
    }

    #[test]
    fn an_empty_draft_shows_no_readout_at_all() {
        assert_eq!(readout_text(0, None, true), None);
    }
}
