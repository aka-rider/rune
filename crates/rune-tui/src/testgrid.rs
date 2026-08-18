//! Shared `TestBackend` -> glyph-grid extractor for headless render tests.
//! Every test module that used to hand-roll its own draw-
//! into-a-`TestBackend`-then-read-cells-back boilerplate (`tests/
//! tui_render.rs` was the reference this module generalises) now goes
//! through here instead — including `src/opentabs.rs`'s and
//! `src/title.rs`'s own test modules, which had grown a
//! second, independent copy of the same construction, the coverage gap
//! that let the "one place" claim below silently rot. `tests/
//! testgrid_inventory.rs` makes the claim self-checking rather than a
//! comment nobody re-verifies: it asserts `TestBackend::new` appears
//! exactly once crate-wide, right here.
//!
//! `draw_with` is the actual common denominator; `draw`/`grid`/`row` are
//! thin convenience wrappers over it. Most callers only need `grid`/`row`;
//! a few need the raw `ratatui::buffer::Buffer`
//! back for cell-level color/modifier assertions (`tests/chrome.rs`'s
//! border-color checks, `tests/tui_render.rs`'s caret/bold-modifier checks)
//! or need to draw something other than the whole `App` (`src/
//! breadcrumb.rs`'s `overlay` unit tests draw a sub-`Rect` directly,
//! bypassing `render::draw`) — `draw`/`draw_with` cover those without
//! reintroducing a tenth hand-rolled copy.

use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer as RtBuffer;

use crate::app::App;
use crate::render;

/// Runs `f` against a fresh `w`x`h` `TestBackend` and returns the resulting
/// buffer — the one place left in the crate that constructs a
/// `TestBackend`. Generic over the draw closure so a caller that needs to
/// render something other than the whole `App` (a single component, into
/// its own `Rect`) still goes through here rather than rolling its own
/// terminal.
#[allow(clippy::expect_used)]
pub fn draw_with(w: u16, h: u16, f: impl FnOnce(&mut Frame)) -> RtBuffer {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("terminal construction");
    terminal.draw(f).expect("draw");
    terminal.backend().buffer().clone()
}

/// Draws `app` into a `w`x`h` `TestBackend` via the real `render::draw` and
/// returns the raw buffer — for callers that need cell-level style (color,
/// modifiers) rather than just text.
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

/// Draws `app` into a `w`x`h` `TestBackend` and returns every row as its
/// own `String`.
pub fn grid(app: &App, w: u16, h: u16) -> Vec<String> {
    let buf = draw(app, w, h);
    (0..h).map(|y| row_text(&buf, y, w)).collect()
}

/// Draws `app` into a `w`x`h` `TestBackend` and returns row `y` only. A
/// height is required to actually draw a frame, so it is taken explicitly
/// here even though only one row is returned.
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
