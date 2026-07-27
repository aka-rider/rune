//! The breadcrumb row: the active document's path, one segment per path
//! component, joined by `\u{25b8}` (plan WP6.S1) — a pure renderer, `fn
//! draw(app: &App, area: Rect, frame: &mut Frame)`, reading `&App` only.
//! Port of Go `pkg/ui/components/breadcrumb/breadcrumb.go`'s segment-join
//! idea, adapted to this crate's own chrome shape: Go overlays the crumb
//! text directly onto the editor pane's bottom BORDER line (`lipgloss`
//! string surgery in `workspace_view.go::overlayBreadcrumb`); this crate's
//! chrome is ratatui rect-based rather than string-surgery-based (WP2), so
//! WP6 gives the breadcrumb its OWN reserved row instead of splicing into a
//! border — same visual language (`styles::Special`-colored text, `\u{2500}`
//! dash fill for the unused width, straight from Go's own `dashFill`), a
//! different layout mechanism. A pathless document (a draft, the Help
//! virtual doc) renders nothing at all — the row stays whatever was already
//! in the frame buffer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::path::Component;

use crate::app::App;
use crate::styles;

/// The segment separator between path components (plan WP6.S1's literal
/// format: `" segment \u{25b8} segment \u{25b8} name "`).
const SEGMENT_SEP: &str = " \u{25b8} ";

/// Renders the active document's `file_path` as `\u{25b8}`-joined segments
/// (only the `Normal` path components — `RootDir`/`CurDir`/`ParentDir` never
/// appear in a segment, matching Go's `filepath.Base`/`Join`-relativized
/// crumb, which never shows a bare `/`), padded with one leading/trailing
/// space, then the row's remaining width filled with `\u{2500}` dashes (Go's
/// `dashFill` convention) — both in `styles::SPECIAL` (Go's `Breadcrumb`
/// style: `Foreground(special)`). Renders nothing for a pathless document.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let doc = app.active_doc();
    let Some(path) = &doc.file_path else {
        return;
    };

    let segments: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if segments.is_empty() {
        return;
    }

    let style = Style::new().fg(styles::SPECIAL);
    let content = format!(" {} ", segments.join(SEGMENT_SEP));
    let content_width = content.chars().count() as u16;
    let fill_width = area.width.saturating_sub(content_width);

    let mut spans = vec![Span::styled(content, style)];
    if fill_width > 0 {
        spans.push(Span::styled("\u{2500}".repeat(fill_width as usize), style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn app_for(content: &str, path: Option<&str>) -> App {
        App::new(
            Buffer::new(content),
            path.map(PathBuf::from),
            Arc::new(Mem::new()),
            None,
        )
    }

    fn draw_line(app: &App, width: u16) -> String {
        let backend = TestBackend::new(width, 1);
        let mut terminal = Terminal::new(backend).expect("terminal construction");
        terminal
            .draw(|frame| draw(app, frame.area(), frame))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let mut s = String::new();
        for x in 0..width {
            if let Some(cell) = buf.cell((x, 0)) {
                s.push_str(cell.symbol());
            }
        }
        s
    }

    #[test]
    fn renders_path_segments_joined_by_the_separator() {
        let app = app_for("hello", Some("/a/b/note.md"));
        let line = draw_line(&app, 40);
        assert!(line.contains("a \u{25b8} b \u{25b8} note.md"), "{line:?}");
    }

    #[test]
    fn pathless_doc_renders_nothing() {
        let app = app_for("hello", None);
        let line = draw_line(&app, 40);
        assert_eq!(line.trim(), "");
    }
}
