use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::pane::Pane;
use crate::projectsearch::query::FileHit;
use crate::projectsearch::{MIN_QUERY_CHARS, ProjectSearchState};
use crate::render::fuzzyspan::{display_spans, with_bg};
use crate::width::display_width;

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let Some(state) = app.projectsearch() else {
        return;
    };
    if area.height == 0 {
        return;
    }

    let bar_area = Rect::new(area.x, area.y, area.width, 1);
    let readout = readout_text(app);
    let spans = crate::render::search::build_spans(
        &state.query,
        readout.as_deref(),
        true,
        area.width as usize,
        &app.theme,
    );
    frame.render_widget(Paragraph::new(Line::from(spans)), bar_area);

    let rows_height = area.height.saturating_sub(1);
    if rows_height == 0 {
        return;
    }
    let rows_area = Rect::new(area.x, area.y + 1, area.width, rows_height);
    let lines = result_lines(app, state, rows_height as usize, area.width as usize);
    frame.render_widget(Paragraph::new(lines), rows_area);
}

fn readout_text(app: &App) -> Option<String> {
    let index = app.project_index.as_ref()?;
    if index.building {
        return Some(crate::projectsearch::spinner_char(index.spinner_frame).to_string());
    }
    let state = app.projectsearch()?;
    if state.query.chars().count() < MIN_QUERY_CHARS {
        return Some("2+ chars".to_string());
    }
    let files = state.results.len();
    if files == 0 {
        return Some("no matches".to_string());
    }
    let approx = if index.truncated || state.results_truncated {
        "\u{2248}"
    } else {
        ""
    };
    let noun = if files == 1 { "file" } else { "files" };
    Some(format!("{approx}{files} {noun}"))
}

fn result_lines(
    app: &App,
    state: &ProjectSearchState,
    rows: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let focused = app.focus() == Pane::Explorer;
    let window = state.list.window(state.results.len(), rows);
    let start = window.start;
    let visible = state.results.get(window).unwrap_or(&[]);
    visible
        .iter()
        .enumerate()
        .map(|(i, hit)| {
            let display = if start + i != state.list.cursor {
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn app() -> crate::app::App {
        let mut app = crate::app::App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        app
    }

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

    #[test]
    fn the_readout_shows_one_spinner_char_only_while_the_build_is_in_flight() {
        let mut app = app();
        let mut effects = crate::runtime::Effects::default();
        crate::projectsearch::open(&mut app, &mut effects);

        let building = readout_text(&app).expect("a build is in flight right after open");
        assert_eq!(building.chars().count(), 1);
        assert!(('\u{2800}'..='\u{28FF}').contains(&building.chars().next().unwrap()));

        if let Some(index) = app.project_index.as_mut() {
            index.building = false;
        }
        assert_eq!(
            readout_text(&app),
            Some("2+ chars".to_string()),
            "an idle index with a short query shows the minimum-length hint"
        );
    }

    #[test]
    fn the_idle_readout_walks_hint_then_no_matches_then_counts() {
        let mut app = app();
        let mut effects = crate::runtime::Effects::default();
        crate::projectsearch::open(&mut app, &mut effects);
        if let Some(index) = app.project_index.as_mut() {
            index.building = false;
        }

        if let Some(state) = app.projectsearch_mut() {
            state.query = "he".to_string();
        }
        assert_eq!(readout_text(&app), Some("no matches".to_string()));

        if let Some(state) = app.projectsearch_mut() {
            state.results = vec![hit("a.md", 3)];
        }
        assert_eq!(readout_text(&app), Some("1 file".to_string()));

        if let Some(state) = app.projectsearch_mut() {
            state.results = vec![hit("a.md", 3), hit("b.md", 1)];
            state.results_truncated = true;
        }
        assert_eq!(readout_text(&app), Some("\u{2248}2 files".to_string()));
    }

    #[test]
    fn result_rows_show_the_path_with_a_right_aligned_count() {
        let mut app = app();
        let mut effects = crate::runtime::Effects::default();
        crate::projectsearch::open(&mut app, &mut effects);
        if let Some(state) = app.projectsearch_mut() {
            state.results = vec![hit("sub/a.md", 12), hit("b.md", 4)];
        }
        let state = app.projectsearch().expect("open");

        let lines = result_lines(&app, state, 5, 20);
        assert_eq!(lines.len(), 2);
        let first: String = lines
            .first()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default();
        assert!(first.starts_with("\u{203a} sub/a.md"));
        assert!(first.ends_with("12"));
        assert_eq!(first.chars().count(), 20, "rows pad to the full width");
        let second: String = lines
            .get(1)
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default();
        assert!(second.starts_with("  b.md"));
        assert!(second.ends_with("4"));
    }
}
