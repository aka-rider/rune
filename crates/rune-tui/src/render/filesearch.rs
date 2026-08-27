use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::filesearch::{FileSearchState, ResultRow, candidate_at};
use crate::pane::Pane;
use crate::render::fuzzyspan::{display_spans, with_bg};
use crate::theme::Theme;
use crate::width::display_width;

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let Some(state) = app.filesearch() else {
        return;
    };
    if area.height == 0 {
        return;
    }

    let bar_area = Rect::new(area.x, area.y, area.width, 1);
    let readout = readout_text(state);
    let spans = crate::render::search::build_spans(
        &state.query,
        Some(readout.as_str()),
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

fn result_lines(
    app: &App,
    state: &FileSearchState,
    rows: usize,
    width: usize,
) -> Vec<Line<'static>> {
    if state.results.is_empty() && !state.query.trim().is_empty() {
        return vec![Line::from(Span::styled(
            "no matches",
            Style::new().fg(app.theme.chrome.subtle),
        ))];
    }

    let focused = app.focus() == Pane::Explorer;
    let window = state.nav.window(state.results.len(), rows);
    let start = window.start;
    let visible = state.results.get(window).unwrap_or(&[]);
    visible
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let display = if start + i != state.nav.cursor {
                RowDisplay::Plain
            } else if focused {
                RowDisplay::CursorFocused
            } else {
                RowDisplay::CursorUnfocused
            };
            result_line(app, state, row, display, width)
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowDisplay {
    Plain,
    CursorFocused,
    CursorUnfocused,
}

fn result_line(
    app: &App,
    state: &FileSearchState,
    row: &ResultRow,
    display: RowDisplay,
    width: usize,
) -> Line<'static> {
    let row_bg = (display == RowDisplay::CursorFocused).then_some(app.theme.chrome.selection_bg);
    let prefix = if display == RowDisplay::Plain {
        "  "
    } else {
        "\u{203a} "
    };
    let mut spans = vec![Span::styled(
        prefix.to_string(),
        with_bg(app.theme.chrome.file_normal, row_bg),
    )];

    if let Some(candidate) = candidate_at(state, row.candidate_idx) {
        let avail = width.saturating_sub(display_width(prefix));
        spans.extend(candidate_spans(
            &candidate.display,
            &row.indices,
            &app.theme,
            avail,
            row_bg,
        ));
    }

    let content_w: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    if width > content_w {
        spans.push(Span::styled(
            " ".repeat(width - content_w),
            with_bg(app.theme.chrome.file_normal, row_bg),
        ));
    }
    Line::from(spans)
}

fn candidate_spans(
    display: &str,
    indices: &[u32],
    theme: &Theme,
    avail_w: usize,
    row_bg: Option<Color>,
) -> Vec<Span<'static>> {
    let dir_end = display.rfind('/').map_or(0, |i| i + 1);
    let dim_style = with_bg(Style::new().fg(theme.chrome.subtle), row_bg);
    let file_style = with_bg(theme.chrome.file_normal, row_bg);
    display_spans(display, indices, dim_style, file_style, avail_w, dir_end)
}

fn readout_text(state: &FileSearchState) -> String {
    if state.walk_pending {
        return "scanning\u{2026}".to_string();
    }
    let matched = state.results.len();
    let total = state.recents.len() + state.walk.len();
    let truncated = if state.walk_truncated {
        "+truncated"
    } else {
        ""
    };
    format!("{matched}/{total}{truncated}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::filesearch::{Candidate, walk};
    use crate::runtime::CmdKind;
    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, VfsTestExt};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        app
    }

    #[test]
    fn readout_shows_scanning_while_the_walk_is_pending() {
        let mut app = app();
        app.root = Some(PathBuf::from("/root"));
        let mut effects = crate::runtime::Effects::default();

        crate::filesearch::open(&mut app, &mut effects);

        assert_eq!(
            readout_text(app.filesearch().expect("open")),
            "scanning\u{2026}".to_string()
        );
    }

    #[test]
    fn readout_shows_matched_over_total_once_the_scan_reply_lands() {
        let mut app = app();
        app.root = Some(PathBuf::from("/root"));
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;

        crate::filesearch::handle_scanned(
            &mut app,
            generation,
            Ok(walk::ScanResult {
                files: Vec::new(),
                truncated: false,
            }),
            &mut effects,
        );

        assert_eq!(
            readout_text(app.filesearch().expect("open")),
            "0/0".to_string()
        );
    }

    #[test]
    fn hand_delivered_scan_reply_lists_recents_then_walk_and_dedups_by_path() {
        let vfs = Mem::new();
        vfs.save_atomic(Path::new("/root/a.md"), b"a")
            .expect("seed a.md");
        vfs.save_atomic(Path::new("/root/b.md"), b"b")
            .expect("seed b.md");
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(vfs), None);
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        app.root = Some(PathBuf::from("/root"));
        let mut effects = crate::runtime::Effects::default();

        crate::filesearch::open(&mut app, &mut effects);
        assert!(
            effects.cmds.iter().any(|c| c.kind() == CmdKind::ReadDir),
            "open pushes the scan Cmd rather than running it inline"
        );
        assert_eq!(
            readout_text(app.filesearch().expect("open")),
            "scanning\u{2026}".to_string()
        );

        let generation = app.filesearch().expect("open").generation;
        if let Some(state) = app.filesearch_mut() {
            state.recents.push(Candidate {
                path: PathBuf::from("/root/a.md"),
                display: "a.md".to_string(),
                in_tree: true,
                mru_rank: Some(0),
            });
        }

        crate::filesearch::handle_scanned(
            &mut app,
            generation,
            Ok(walk::ScanResult {
                files: vec![PathBuf::from("/root/a.md"), PathBuf::from("/root/b.md")],
                truncated: false,
            }),
            &mut effects,
        );

        let state = app.filesearch().expect("still open");
        assert_eq!(
            state
                .walk
                .iter()
                .map(|c| c.path.clone())
                .collect::<Vec<_>>(),
            vec![PathBuf::from("/root/b.md")],
            "a.md is already covered by a recent, so it's dropped from walk"
        );
        assert_eq!(state.results.len(), 2);
        assert_eq!(
            state
                .results
                .first()
                .and_then(|r| crate::filesearch::candidate_at(state, r.candidate_idx))
                .map(|c| c.path.clone()),
            Some(PathBuf::from("/root/a.md")),
            "recents occupy the low flat indices, ahead of walk"
        );
        assert_eq!(
            state
                .results
                .get(1)
                .and_then(|r| crate::filesearch::candidate_at(state, r.candidate_idx))
                .map(|c| c.path.clone()),
            Some(PathBuf::from("/root/b.md"))
        );
        assert_eq!(readout_text(state), "2/2".to_string());
    }

    #[test]
    fn draw_is_a_no_op_when_the_finder_is_closed() {
        let app = app();
        assert!(app.filesearch().is_none());
        let buf = crate::testgrid::draw_with(20, 5, |frame| {
            draw(&app, Rect::new(0, 0, 20, 5), frame);
        });
        for y in 0..5 {
            for x in 0..20 {
                let cell = buf.cell((x, y)).expect("cell in bounds");
                assert_eq!(cell.symbol(), " ");
            }
        }
    }

    fn candidate(path: &str, display: &str) -> Candidate {
        Candidate {
            path: PathBuf::from(path),
            display: display.to_string(),
            in_tree: true,
            mru_rank: None,
        }
    }

    fn seeded_app(files: &[(&str, &str)]) -> App {
        let mem = Mem::new();
        for (path, content) in files {
            mem.save_atomic(Path::new(path), content.as_bytes())
                .expect("seed file");
        }
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(mem), None);
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        app.root = Some(PathBuf::from("/root"));
        app
    }

    #[test]
    fn a_query_with_no_matches_renders_an_explicit_empty_state_row() {
        let mut app = seeded_app(&[("/root/a.md", "a")]);
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;
        crate::filesearch::handle_recents_loaded(
            &mut app,
            generation,
            Ok(vec![candidate("/root/a.md", "a.md")]),
            &mut effects,
        );
        if let Some(state) = app.filesearch_mut() {
            state.query = "zzzzzznomatch".to_string();
        }
        crate::filesearch::recompute(&mut app, &mut effects);
        let state = app.filesearch().expect("still open");
        assert!(state.results.is_empty(), "test setup: nothing matches");

        let lines = result_lines(&app, state, 5, 40);
        let text: String = lines
            .first()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default();
        assert_eq!(text, "no matches");
    }

    #[test]
    fn multi_row_highlight_indices_are_disjoint_and_per_row_correct() {
        let mut app = seeded_app(&[("/root/note-a.md", "a"), ("/root/note-b.md", "b")]);
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;
        crate::filesearch::handle_recents_loaded(
            &mut app,
            generation,
            Ok(vec![
                candidate("/root/note-a.md", "note-a.md"),
                candidate("/root/note-b.md", "note-b.md"),
            ]),
            &mut effects,
        );
        if let Some(state) = app.filesearch_mut() {
            state.query = "note".to_string();
        }
        crate::filesearch::recompute(&mut app, &mut effects);
        let state = app.filesearch().expect("still open");
        assert_eq!(state.results.len(), 2, "both candidates match \"note\"");

        for row in &state.results {
            let candidate = candidate_at(state, row.candidate_idx).expect("row names a candidate");
            assert_eq!(
                row.indices,
                vec![0, 1, 2, 3],
                "each row's own indices must name ITS OWN \"note\" prefix, \
                 not a leaked copy from the other row: {}",
                candidate.display
            );
        }
    }

    #[test]
    fn nfd_decomposed_filename_bolds_the_matching_ascii_tail_graphemes() {
        let nfd_name = "cafe\u{0301}.md";
        let mut app = seeded_app(&[]);
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;
        crate::filesearch::handle_recents_loaded(
            &mut app,
            generation,
            Ok(vec![candidate("/root/cafe.md", nfd_name)]),
            &mut effects,
        );
        if let Some(state) = app.filesearch_mut() {
            state.query = "md".to_string();
        }
        crate::filesearch::recompute(&mut app, &mut effects);
        let state = app.filesearch().expect("still open");
        let row = state
            .results
            .first()
            .expect("query \"md\" matches the NFD filename's ascii tail");

        let spans = candidate_spans(nfd_name, &row.indices, &app.theme, 80, None);
        let bold_graphemes: Vec<String> = spans
            .iter()
            .filter(|s| {
                s.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::BOLD)
            })
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(bold_graphemes, vec!["m".to_string(), "d".to_string()]);

        let non_bold_graphemes: Vec<String> = spans
            .iter()
            .filter(|s| {
                !s.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::BOLD)
            })
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
            ],
            "the NFD e + combining-acute grapheme stays intact as one unmatched span"
        );
    }
}
