//! The messages pane's own row builder: one separator row (styled by
//! whether the pane holds focus), then the log document's rows through the
//! SAME `render::build_rows` the editor uses — required for mouse
//! hit-testing to ever land correctly, since the old `banner` module's own
//! row walk built a different row space that hit-testing couldn't reuse —
//! with one extra pass tinting each entry's byte range by its severity.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::pane::Pane;
use crate::render::{self, Cell};
use crate::theme::Theme;

use super::Severity;

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    if area.height == 0 {
        return;
    }
    let sep_style = if app.focus() == Pane::Messages {
        app.theme.chrome.active_border
    } else {
        app.theme.chrome.inactive_border
    };
    let sep_area = Rect::new(area.x, area.y, area.width, 1);
    let separator = "\u{2500}".repeat(area.width as usize);
    frame.render_widget(Paragraph::new(separator).style(sep_style), sep_area);

    if area.height <= 1 {
        return;
    }
    let content_area = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    let Some(view) = app.messages.doc.view.as_ref() else {
        return;
    };
    let mut rows = render::build_rows(app, &app.messages.doc, view);
    apply_severity_colours(&mut rows, &app.theme, &app.messages.ranges);
    render::blit(&rows, content_area, frame);
}

fn severity_style(theme: &Theme, severity: Severity) -> Option<Style> {
    match severity {
        Severity::Error => Some(theme.chrome.error),
        Severity::Warn => Some(theme.chrome.warn),
        Severity::Info => None,
    }
}

/// Mirrors `render::overlay::highlight_selection`'s shape: walk every real
/// cell and, if its byte offset falls inside one of `ranges`, patch in that
/// entry's severity style. `ranges` is small (capped by `MAX_ENTRIES`), so a
/// linear scan per cell is cheap against a pane that only ever shows a
/// handful of rows at once.
fn apply_severity_colours(
    rows: &mut [Vec<Cell>],
    theme: &Theme,
    ranges: &[(std::ops::Range<usize>, Severity)],
) {
    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            if cell.buf_offset < 0 {
                continue;
            }
            let offset = cell.buf_offset as usize;
            let hit = ranges.iter().find(|(range, _)| range.contains(&offset));
            if let Some((_, severity)) = hit
                && let Some(style) = severity_style(theme, *severity)
            {
                cell.style = cell.style.patch(style);
            }
        }
    }
}
