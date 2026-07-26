//! The status line: file name, dirty dot, message area, quit hint (plan
//! Context, "Quit-confirm": "Status line shows 'press again to quit —
//! unsaved changes will be lost' when dirty").

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::app::App;

const DIRTY_DOT: char = '\u{2022}';

/// The status line's text content, split out from `draw` so tests can
/// assert on it without a real `Frame`/`TestBackend`.
pub fn status_text(app: &App) -> String {
    let mut name = app.file_name().to_string();
    if app.is_dirty() {
        name.push(' ');
        name.push(DIRTY_DOT);
    }

    let mut parts = vec![name];
    if let Some(msg) = &app.status_message {
        parts.push(msg.clone());
    }
    if app.pending_quit.is_some() {
        parts.push(quit_hint(app).to_string());
    }
    parts.join("  ")
}

fn quit_hint(app: &App) -> &'static str {
    if app.is_dirty() {
        "press again to quit \u{2014} unsaved changes will be lost"
    } else {
        "press again to quit"
    }
}

pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let style = Style::default().fg(Color::Black).bg(Color::Gray);
    let line = Line::styled(status_text(app), style);
    frame.render_widget(Paragraph::new(line).style(style), area);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()))
    }

    #[test]
    fn clean_buffer_has_no_dirty_dot() {
        let app = app_with("hello");
        assert!(!status_text(&app).contains(DIRTY_DOT));
    }

    #[test]
    fn dirty_buffer_shows_dot_and_pending_quit_shows_hint() {
        let mut app = app_with("hello");
        app.editor.buffer = app.editor.buffer.insert(0, "x");
        assert!(status_text(&app).contains(DIRTY_DOT));

        app.pending_quit = Some((crate::keymap::QuitKey::CtrlC, 0));
        assert!(status_text(&app).contains("press again to quit"));
        assert!(status_text(&app).contains("unsaved changes will be lost"));
    }

    #[test]
    fn no_name_shown_for_unnamed_draft() {
        let app = app_with("");
        assert!(status_text(&app).contains("[No Name]"));
    }
}
