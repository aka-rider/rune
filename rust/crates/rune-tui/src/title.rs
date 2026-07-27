//! The title row: the active document's display name plus a dirty dot (plan
//! WP6.S1) — a pure renderer, `fn draw(app: &App, area: Rect, frame: &mut
//! Frame)`, reading `&App` only, no state of its own (same shape as
//! `footer::draw`). Port of Go `pkg/ui/components/title/title.go`'s
//! unfocused `View()` (the focused/rename-editable path is out of scope
//! here per the plan's "Out of scope: rename/Title focus" — this only
//! renders the read-only display, never edits it).
//!
//! `Document::file_name()` already implements the exact display-name rule
//! this needs (`file_path`'s file-name component, or the Go-parity
//! `"[No Name]"` fallback — WP1) — reused here rather than re-derived.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::styles;

/// Renders `<name>` (styled `styles::title_text` — Go's `TitleText`, color
/// 216, bold) followed by ` •` (`styles::error`, Go's dirty-tab convention)
/// when the active document is dirty. Pure function of `&App` — reads
/// `app.active_doc()` fresh every call, no cached/derived state.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let doc = app.active_doc();
    let mut spans = vec![Span::styled(
        doc.file_name().to_string(),
        styles::title_text(),
    )];
    if doc.is_dirty() {
        spans.push(Span::styled(" \u{2022}", styles::error()));
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
    use std::sync::Arc;

    fn app_for(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
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
    fn no_name_placeholder_when_pathless() {
        let app = app_for("hello");
        assert!(draw_line(&app, 40).contains("[No Name]"));
    }

    #[test]
    fn dirty_dot_appears_only_when_dirty() {
        let mut app = app_for("hello");
        assert!(!draw_line(&app, 40).contains('\u{2022}'));
        app.active_doc_mut().mark_dirty_from_hydration();
        assert!(draw_line(&app, 40).contains('\u{2022}'));
    }
}
