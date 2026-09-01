use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::pane::Pane;
use crate::width::{display_width, truncate_to_width};

pub fn draw_divider(app: &App, area: Rect, frame: &mut Frame) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let style = if app.focus() == Pane::Tabs {
        app.theme.chrome.active_border
    } else {
        app.theme.chrome.tabs_divider
    };

    let total = area.width as usize;
    let label = truncate_to_width(" Open ", total);
    let fill = "\u{2500}".repeat(total.saturating_sub(display_width(&label)));

    let row = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(label, style),
            Span::styled(fill, style),
        ])),
        row,
    );
}

fn clip_tab_name(name: &str, prefix_width: usize, area_width: u16) -> String {
    let budget = (area_width as usize).saturating_sub(prefix_width);
    truncate_to_width(name, budget)
}

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    if area.height == 0 {
        return;
    }
    let mut lines = Vec::with_capacity(area.height as usize);
    let show_cursor = app.focus() == Pane::Tabs;
    let mut cursor_row: Option<u16> = None;
    let mut active_row: Option<u16> = None;

    for (row_y, &(idx, id, doc)) in super::painted_tabs(app, area).iter().enumerate() {
        let selected = idx == app.tabs.nav.cursor;
        let row_y = row_y as u16;
        if selected {
            cursor_row = Some(row_y);
        }
        if id == app.active {
            active_row = Some(row_y);
        }
        let shortcut = (idx + 1) % 10;

        let prefix = if show_cursor && selected {
            "\u{203a} "
        } else {
            "  "
        };
        let dirty_marker = if doc.is_dirty() { "x" } else { " " };
        let pin_marker = if doc.pinned { "*" } else { " " };
        let sync_marker = if doc
            .last_sync
            .is_some_and(rune_db::SyncKind::is_disk_divergent)
        {
            "\u{21c4}"
        } else {
            " "
        };
        let name_style = if id == app.active {
            app.theme.chrome.tab_active
        } else {
            app.theme.chrome.tab_normal
        };

        let shortcut_label = format!("{shortcut}:");
        let prefix_width = display_width(prefix)
            + display_width(&shortcut_label)
            + display_width(pin_marker)
            + display_width(dirty_marker)
            + display_width(sync_marker)
            + 1;
        let name = clip_tab_name(doc.file_name(), prefix_width, area.width);

        lines.push(Line::from(vec![
            Span::raw(prefix),
            Span::styled(shortcut_label, app.theme.chrome.tabs_divider),
            Span::styled(pin_marker, app.theme.chrome.tab_pinned),
            Span::styled(dirty_marker, app.theme.chrome.tab_dirty),
            Span::styled(sync_marker, app.theme.chrome.error),
            Span::raw(" "),
            Span::styled(name, name_style),
        ]));
    }

    frame.render_widget(Paragraph::new(lines).style(Style::default()), area);

    if let Some(y) = active_row {
        let row = Rect::new(area.x, area.y + y, area.width, 1);
        crate::render::rowbg::fill_row(frame, row, app.theme.chrome.row_active_bg);
    }
    if show_cursor && let Some(y) = cursor_row {
        let row = Rect::new(area.x, area.y + y, area.width, 1);
        crate::render::rowbg::fill_row(frame, row, app.theme.chrome.row_cursor_bg);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::app::App;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.active_doc_mut().viewport.set_size(80, 23);
        app
    }

    fn divider_row(app: &App, width: u16) -> String {
        let buf = crate::testgrid::draw_with(width, 1, |frame| {
            draw_divider(app, Rect::new(0, 0, width, 1), frame)
        });
        (0..width)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()))
            .collect()
    }

    #[test]
    fn the_divider_fills_its_whole_width_after_the_label() {
        let app = app();
        assert_eq!(
            divider_row(&app, 20),
            format!(" Open {}", "\u{2500}".repeat(14))
        );
    }

    #[test]
    fn a_narrow_divider_truncates_the_label_instead_of_overflowing() {
        let app = app();
        for width in 1u16..=6 {
            let row = divider_row(&app, width);
            assert_eq!(
                row.chars().count(),
                width as usize,
                "width {width} must render exactly {width} cells: {row:?}"
            );
            assert!(
                " Open ".starts_with(&row) || row.chars().all(|c| c == '\u{2500}' || c == ' '),
                "width {width} must render a prefix of the label: {row:?}"
            );
        }
        assert_eq!(divider_row(&app, 6), " Open ");
    }

    #[test]
    fn a_zero_width_divider_draws_nothing() {
        let app = app();
        assert_eq!(divider_row(&app, 0), "");
    }

    #[test]
    fn clip_tab_name_truncates_a_multi_cell_grapheme_name_at_a_grapheme_boundary() {
        let name = "\u{4e2d}\u{6587}\u{4e2d}\u{6587}report.md";
        assert_eq!(clip_tab_name(name, 8, 9), "");
        assert_eq!(clip_tab_name(name, 8, 10), "\u{4e2d}");
        assert_eq!(clip_tab_name(name, 8, 12), "\u{4e2d}\u{6587}");
        assert_eq!(clip_tab_name(name, 8, 200), name);
    }

    #[test]
    fn a_wide_grapheme_document_name_renders_without_panicking() {
        let mut app = app();
        app.active_doc_mut().display_name =
            Some("\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600}\u{1f600} report.md".to_string());
        for width in 1u16..=20 {
            let area = Rect::new(0, 0, width, 1);
            let _ = crate::testgrid::draw_with(width, 1, |frame| draw(&app, area, frame));
        }
    }
}
