use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::find::Control;
use crate::find::project::ProjectResults;
use crate::projectsearch::query::FileHit;
use crate::render::fuzzyspan::{display_spans, with_bg};
use crate::width::display_width;

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let Some(state) = app.find() else {
        return;
    };
    let Some(project) = state.project.as_ref() else {
        return;
    };
    if area.height == 0 {
        return;
    }
    let focused = state.focused && state.focus == Control::Results;
    let lines = result_lines(
        app,
        project,
        focused,
        area.height as usize,
        area.width as usize,
    );
    frame.render_widget(Paragraph::new(lines), area);
}

fn result_lines(
    app: &App,
    project: &ProjectResults,
    focused: bool,
    rows: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let window = project.list.window(project.results.len(), rows);
    let start = window.start;
    let visible = project.results.get(window).unwrap_or(&[]);
    visible
        .iter()
        .enumerate()
        .map(|(i, hit)| {
            let display = if start + i != project.list.cursor {
                RowDisplay::Plain
            } else if focused {
                RowDisplay::CursorFocused
            } else {
                RowDisplay::CursorUnfocused
            };
            result_line(app, hit, display, width)
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowDisplay {
    Plain,
    CursorFocused,
    CursorUnfocused,
}

fn result_line(app: &App, hit: &FileHit, display: RowDisplay, width: usize) -> Line<'static> {
    let row_bg = (display == RowDisplay::CursorFocused).then_some(app.theme.chrome.selection_bg);
    let prefix = if display == RowDisplay::Plain {
        "  "
    } else {
        "\u{203a} "
    };
    let count = hit.count.to_string();
    let dim_style = with_bg(Style::new().fg(app.theme.chrome.subtle), row_bg);
    let file_style = with_bg(app.theme.chrome.file_normal, row_bg);
    let mut spans = vec![Span::styled(prefix.to_string(), file_style)];

    let avail = width
        .saturating_sub(display_width(prefix))
        .saturating_sub(count.len() + 1);
    let dir_end = hit.display.rfind('/').map_or(0, |i| i + 1);
    spans.extend(display_spans(
        &hit.display,
        &[],
        dim_style,
        file_style,
        avail,
        dir_end,
    ));

    let content_w: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let pad = width.saturating_sub(content_w + count.len());
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), file_style));
    }
    spans.push(Span::styled(count, dim_style));
    Line::from(spans)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::find::test_support::{app_with, open_project_find};
    use crate::listnav::List;
    use std::path::PathBuf;

    fn hit(display: &str, count: usize) -> FileHit {
        FileHit {
            path: PathBuf::from(format!("/root/{display}")),
            display: display.to_string(),
            count,
            first_match: 0,
            line: 1,
            ranges: std::iter::once(0..1).collect(),
        }
    }

    fn results(hits: Vec<FileHit>) -> ProjectResults {
        ProjectResults {
            results: hits,
            truncated: false,
            list: List { cursor: 0, top: 0 },
            query_generation: crate::generation::ProjectSearchGen::from_raw(1),
            pending_center: None,
        }
    }

    fn text_of(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn result_rows_show_the_path_with_a_right_aligned_count() {
        let app = app_with("hello");
        let project = results(vec![hit("sub/a.md", 12), hit("b.md", 4)]);

        let lines = result_lines(&app, &project, true, 5, 20);
        assert_eq!(lines.len(), 2);
        let first = text_of(&lines[0]);
        assert!(first.starts_with("\u{203a} sub/a.md"));
        assert!(first.ends_with("12"));
        assert_eq!(first.chars().count(), 20, "rows pad to the full width");
        let second = text_of(&lines[1]);
        assert!(second.starts_with("  b.md"));
        assert!(second.ends_with("4"));
    }

    #[test]
    fn the_cursor_row_carries_the_selection_background_only_while_results_hold_the_ring() {
        let app = app_with("hello");
        let project = results(vec![hit("a.md", 1)]);
        let selection = Some(app.theme.chrome.selection_bg);

        let focused = result_lines(&app, &project, true, 5, 20);
        assert!(focused[0].spans.iter().all(|s| s.style.bg == selection));
        let kept = result_lines(&app, &project, false, 5, 20);
        assert!(kept[0].spans.iter().all(|s| s.style.bg.is_none()));
    }

    #[test]
    fn the_results_list_is_drawn_from_the_first_row_of_the_left_column() {
        let mut app = app_with("hello");
        app.set_root(PathBuf::from("/root"));
        open_project_find(&mut app);
        if let Some(project) = app.find_mut().and_then(|s| s.project.as_mut()) {
            project.results = vec![hit("a.md", 1)];
        }
        let geo = crate::layout::geometry(app.frame_area(), &app);
        app.sync_view();
        let rows = crate::testgrid::grid(&app, app.frame_width(), app.frame_height());
        let row = rows
            .get(usize::from(geo.explorer_inner.y))
            .cloned()
            .unwrap_or_default();
        assert!(row.contains("a.md"), "first list row: {row:?}");
    }
}
