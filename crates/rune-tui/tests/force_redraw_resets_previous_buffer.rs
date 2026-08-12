//! Issue #16, gate 2: proves `Terminal::resize` — the replacement
//! `Guard::force_redraw` now calls instead of `Terminal::clear` — actually
//! invalidates ratatui's diff so the next flush repaints every cell, not
//! just the ones that changed. Built directly against `ratatui`'s own
//! `TestBackend`, no `Guard` involved (`Guard` cannot be constructed
//! without a live terminal, see `crates/rune-tui/src/term.rs`'s own tests).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::layout::{Position, Rect, Size};
use std::convert::Infallible;

/// Wraps `TestBackend`, counting how many `(x, y, &Cell)` triples each
/// `draw` call actually receives — the number ratatui's diff decided had
/// changed. Everything else forwards straight through.
struct CountingBackend {
    inner: TestBackend,
    last_draw_count: usize,
}

impl CountingBackend {
    fn new(width: u16, height: u16) -> Self {
        Self {
            inner: TestBackend::new(width, height),
            last_draw_count: 0,
        }
    }
}

impl Backend for CountingBackend {
    type Error = Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        let mut count = 0;
        let cells: Vec<_> = content.inspect(|_| count += 1).collect();
        self.last_draw_count = count;
        self.inner.draw(cells.into_iter())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

fn draw_frame_a(terminal: &mut Terminal<CountingBackend>) {
    terminal
        .draw(|frame| {
            frame.render_widget("hello", frame.area());
        })
        .expect("draw frame A");
}

#[test]
fn resize_to_the_same_area_forces_a_full_repaint() {
    let backend = CountingBackend::new(10, 3);
    let mut terminal = Terminal::new(backend).expect("construct terminal");

    draw_frame_a(&mut terminal);
    let first_draw_count = terminal.backend().last_draw_count;
    assert!(
        first_draw_count > 0,
        "the first draw of frame A must paint something"
    );

    draw_frame_a(&mut terminal);
    assert_eq!(
        terminal.backend().last_draw_count,
        0,
        "redrawing an unchanged frame A must produce an empty diff"
    );

    let area = terminal.size().expect("terminal size");
    terminal
        .resize(Rect::from(area))
        .expect("resize to the same area invalidates the back buffer");

    draw_frame_a(&mut terminal);
    assert_eq!(
        terminal.backend().last_draw_count,
        first_draw_count,
        "after an invalidating resize, redrawing frame A must repaint every \
         cell frame A ever painted, not rely on the stale diff"
    );
}
