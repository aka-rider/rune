use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer as RtBuffer;

use crate::app::App;
use crate::render;

// The one place in the crate that constructs a `TestBackend`; generic over
// the draw closure so a caller rendering a single component into its own
// `Rect`, not the whole `App`, still goes through here.
#[allow(clippy::expect_used)]
pub fn draw_with(w: u16, h: u16, f: impl FnOnce(&mut Frame)) -> RtBuffer {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("terminal construction");
    terminal.draw(f).expect("draw");
    terminal.backend().buffer().clone()
}

// Returns the raw buffer, for a caller that needs cell-level style (color,
// modifiers) rather than just text.
pub fn draw(app: &App, w: u16, h: u16) -> RtBuffer {
    draw_with(w, h, |frame| render::draw(app, frame))
}

fn row_text(buf: &RtBuffer, y: u16, w: u16) -> String {
    let mut s = String::new();
    for x in 0..w {
        if let Some(cell) = buf.cell((x, y)) {
            s.push_str(cell.symbol());
        }
    }
    s
}

pub fn grid(app: &App, w: u16, h: u16) -> Vec<String> {
    let buf = draw(app, w, h);
    (0..h).map(|y| row_text(&buf, y, w)).collect()
}

// `h` is taken explicitly even though only row `y` is returned: a frame
// still needs a real height to draw at all.
pub fn row(app: &App, y: u16, w: u16, h: u16) -> String {
    row_text(&draw(app, w, h), y, w)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_for(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    #[test]
    fn grid_returns_h_rows_of_w_chars() {
        let app = app_for("hello");
        let g = grid(&app, 20, 5);
        assert_eq!(g.len(), 5);
        for r in &g {
            assert_eq!(r.chars().count(), 20);
        }
    }

    #[test]
    fn draw_with_renders_an_arbitrary_closure_not_just_render_draw() {
        use ratatui::layout::Rect;
        use ratatui::widgets::{Block, Widget};

        let buf = draw_with(10, 3, |frame| {
            Block::bordered().render(Rect::new(0, 0, 10, 3), frame.buffer_mut());
        });
        let corner = buf.cell((0, 0)).map(ratatui::buffer::Cell::symbol);
        assert_ne!(
            corner,
            Some(" "),
            "expected a border glyph drawn by the closure, not a blank cell"
        );
    }
}
