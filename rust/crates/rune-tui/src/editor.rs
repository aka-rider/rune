//! Editor state: buffer, cursors, the display-pipeline root machine, and the
//! scrollable viewport onto it. Owned by `App`; `Editor::sync` is the fixed
//! per-message sync sequence (plan Context, "Msg/Cmd runtime": `sync_content`
//! iff version changed -> `set_width` -> `sync_cursors` -> `snapshot` ->
//! scroll-to-cursor).

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_core::undo::Journal;
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
    /// The in-memory undo/redo journal (WP7): every applied edit batch is
    /// pushed here as a `Step`; `commands::edit::undo`/`redo` peek-then-
    /// commit against it (plan Context, "Undo journal").
    pub journal: Journal,
    /// Guards `commands::clipboard::handle_paste_content` against mutating a
    /// read-only document (plan Gotchas port of `commands_clipboard.go:153-
    /// 181`'s `m.readOnly` guard — the Go bug it closes: paste bypassing the
    /// read-only check that every keyboard-insert path already had). Phase 1
    /// defines no read-only document (Go's Help view is workspace/Phase-4
    /// scope), so this is always `false` today — the field exists so a
    /// future read-only view only needs to set it, not re-plumb the guard
    /// into every paste source again.
    pub read_only: bool,
}

impl Editor {
    pub fn new(buffer: Buffer) -> Editor {
        Editor {
            buffer,
            cursors: CursorSet::new(0),
            doc: DocMachine::new(),
            viewport: Viewport::default(),
            focused: true,
            journal: Journal::new(),
            read_only: false,
        }
    }

    /// The pure QUERY half of the per-message sync sequence (plan Context,
    /// "Msg/Cmd runtime"): `sync_content` iff version changed -> `set_width`
    /// -> `sync_cursors` -> `snapshot`. Deliberately does NOT touch
    /// `viewport.scroll_row` — see `scroll_to_cursor`'s docs (review finding
    /// F4: separating the snapshot-returning query from the scroll
    /// mutation removes the double-write/double-computation `sync` used to
    /// cause).
    ///
    /// Idempotent/cheap when nothing changed — `sync_content`/
    /// `sync_cursors` are no-ops in that case (plan Gotchas: "Reveal must
    /// never bump the buffer version") — so `commands::nav`/`commands::edit`
    /// call this freely, more than once per message batch, to get
    /// Buffer<->Syntax<->Wrap coordinate conversions that reflect the
    /// CURRENT `Editor` fields (in particular a `Resize` already applied
    /// earlier in the same batch — see their module docs) before computing
    /// a new cursor position.
    pub fn view(&mut self) -> ViewSnapshots {
        self.doc.set_focus(self.focused);
        self.doc.sync_content(&self.buffer);
        self.doc.set_width(self.viewport.width);
        self.doc.sync_cursors(&self.buffer, &self.cursors);
        self.doc.snapshot(&self.buffer)
    }

    /// Scrolls the viewport so the PRIMARY cursor's current row is visible.
    /// The single writer of `viewport.scroll_row` (review finding F4: "no
    /// shadow state" — a value has exactly one writer). Callers that only
    /// need coordinate conversions (`commands::nav`/`commands::edit`) must
    /// use `view()` instead and never call this themselves: calling it
    /// mid-motion would scroll toward a cursor position that's about to
    /// change again later in the same batch, then get silently overwritten
    /// by the batch's real settle — wasted work at best, a visibly wrong
    /// intermediate scroll at worst.
    pub fn scroll_to_cursor(&mut self, view: &ViewSnapshots) {
        let primary = self.cursors.primary();
        let buffer_point = self.buffer.offset_to_line_col(primary.position);
        let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
        self.viewport.scroll_to_row(wrap_point.row);
    }

    /// The fixed per-BATCH settle sequence: rebuild the view, then scroll to
    /// the (by now final) cursor exactly once. `App::sync_view` — called
    /// once per whole message batch by the runtime (`runtime::run`) and by
    /// tests that need the settled state — is the only caller; movement/
    /// editing commands call `view()` alone (see its docs).
    pub fn sync(&mut self) -> ViewSnapshots {
        let view = self.view();
        self.scroll_to_cursor(&view);
        view
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
