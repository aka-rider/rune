//! Editor state: buffer, cursors, the display-pipeline root machine, and the
//! scrollable viewport onto it. Owned by `App`; `Editor::sync` is the fixed
//! per-message sync sequence (plan Context, "Msg/Cmd runtime": `sync_content`
//! iff version changed -> `set_width` -> `sync_cursors` -> `snapshot` ->
//! scroll-to-cursor).

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::{DocMachine, ViewSnapshots};

/// The visible window onto the wrapped document: `width`/`height` in cells,
/// `scroll_row` the first visible wrap row (plan Context, "Cell model" /
/// coords.rs `WrapRow`).
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
    pub scroll_row: usize,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            width: 80,
            height: 24,
            scroll_row: 0,
        }
    }
}

impl Viewport {
    pub fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    /// Clamp `scroll_row` so wrap row `row` is visible — the scroll-to-
    /// cursor step of the per-message sync sequence.
    pub fn scroll_to_row(&mut self, row: usize) {
        let height = self.height as usize;
        if height == 0 {
            return;
        }
        if row < self.scroll_row {
            self.scroll_row = row;
        } else if row >= self.scroll_row + height {
            self.scroll_row = row + 1 - height;
        }
    }
}

/// Buffer + cursors + the root display machine + the viewport onto it.
/// `pending_quit` state lives on `App` (it's app-wide, not editor-specific);
/// this struct holds only the state a single editing pane needs.
pub struct Editor {
    pub buffer: Buffer,
    pub cursors: CursorSet,
    pub doc: DocMachine,
    pub viewport: Viewport,
    pub focused: bool,
}

impl Editor {
    pub fn new(buffer: Buffer) -> Editor {
        Editor {
            buffer,
            cursors: CursorSet::new(0),
            doc: DocMachine::new(),
            viewport: Viewport::default(),
            focused: true,
        }
    }

    /// The fixed per-message sync sequence (plan Context, "Msg/Cmd
    /// runtime"): `sync_content` iff version changed -> `set_width` ->
    /// `sync_cursors` -> `snapshot` -> scroll-to-cursor via
    /// `syntax_to_wrap`. Idempotent when nothing changed — `sync_content`/
    /// `sync_cursors` are no-ops in that case (plan Gotchas: "Reveal must
    /// never bump the buffer version"), so calling this more than once per
    /// message batch is cheap and safe.
    pub fn sync(&mut self) -> ViewSnapshots {
        self.doc.set_focus(self.focused);
        self.doc.sync_content(&self.buffer);
        self.doc.set_width(self.viewport.width);
        self.doc.sync_cursors(&self.buffer, &self.cursors);
        let snapshot = self.doc.snapshot(&self.buffer);

        let primary = self.cursors.primary();
        let buffer_point = self.buffer.offset_to_line_col(primary.position);
        let syntax_point = snapshot.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = snapshot.wrap.syntax_to_wrap(syntax_point);
        self.viewport.scroll_to_row(wrap_point.row);

        snapshot
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn sync_reparses_once_and_is_idempotent_on_repeat_calls() {
        let mut ed = Editor::new(Buffer::new("# hello\nworld\n"));
        ed.viewport.set_size(80, 24);
        let first = ed.sync();
        // "# hello" + "world" + the trailing empty line from the final \n.
        assert_eq!(first.display.total_rows, 3);
        let second = ed.sync();
        assert_eq!(second.display.total_rows, first.display.total_rows);
    }

    #[test]
    fn scroll_to_row_keeps_row_in_view() {
        let mut vp = Viewport {
            width: 80,
            height: 5,
            scroll_row: 0,
        };
        vp.scroll_to_row(10);
        assert_eq!(vp.scroll_row, 6); // 10 + 1 - 5
        vp.scroll_to_row(2);
        assert_eq!(vp.scroll_row, 2); // scrolled back up to keep row 2 visible
    }
}
