//! Renders the search bar's one row: the live draft on the left, a
//! right-aligned readout (`i/N`, `N matches`, or `no matches`) on the
//! right. A sibling of `search` (the state module), not a descendant — it
//! reads `SearchState` only through the `pub(crate)` fields that module
//! marks for exactly this.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::width::{display_width, truncate_to_width};

/// Pure function of `&App`: drawing twice produces identical output, the
/// same guarantee `render::title::draw` makes. A no-op if the bar isn't
/// open — `render::draw` already gates this call on `Geometry::search_bar`
/// being `Some`, which itself requires `App::search` to be `Some`, but the
/// guard stays here too rather than trusting that chain silently.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let Some(state) = app.search.as_ref() else {
        return;
    };
    let theme = &app.theme;

    let readout = readout_text(
        state.matches.len(),
        state.current,
        state.draft.trim().is_empty(),
    );
    let readout_w = readout.as_deref().map(display_width).unwrap_or(0);
    let area_w = area.width as usize;
    let gap = usize::from(readout_w > 0 && area_w > readout_w);
    let draft_budget = area_w.saturating_sub(readout_w + gap);
    let draft_shown = truncate_to_width(&state.draft, draft_budget);
    let draft_w = display_width(&draft_shown);
    let pad = area_w.saturating_sub(draft_w + readout_w);

    let mut line = String::with_capacity(area_w);
    line.push_str(&draft_shown);
    for _ in 0..pad {
        line.push(' ');
    }
    if let Some(readout) = &readout {
        line.push_str(readout);
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(line, theme.chrome.title_text))),
        area,
    );
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
