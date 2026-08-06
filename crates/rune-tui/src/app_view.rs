//! `App::relayout`/`App::sync_view` — the geometry-and-display settle step
//! every message batch runs before the next draw (split out of `app.rs`
//! for the 500-line budget). Still plain `impl App` methods; nothing about
//! how they are reached from the rest of the crate changes.

use crate::app::App;
use crate::pane::Pane;

impl App {
    /// The ONE geometry chokepoint's writer (plan WP3 decision 1/2): derives
    /// every frame rect from `layout::geometry` and sizes the ACTIVE
    /// document's viewport from its `editor` rect. A no-op while either
    /// frame dimension is still `0` (before the first `Msg::Resize` —
    /// `layout::geometry` would otherwise be asked to lay out a
    /// zero-by-something frame it never actually has to render).
    ///
    /// Called from `sync_view` below (the runtime calls `sync_view`
    /// immediately before every `render::draw`, so that's the one chokepoint
    /// no call site can forget) AND again from `Msg::Resize` itself, so tests
    /// that call `update` without a following `sync_view` still see a
    /// correctly-sized viewport; calling it twice in the same message batch is
    /// harmless.
    ///
    /// NOT a pure function of the frame dimensions alone: the message pane's
    /// rect is carved from the same frame, and its height is read off the log
    /// document's cached view. This is idempotent only once that view has been
    /// re-synced for the current frame width and log contents — which is why
    /// `sync_view` syncs the pane BEFORE calling this, and why `Msg::Resize`
    /// is followed by a `sync_view` of its own. Size the viewport from a stale
    /// pane height and the editor's cached rows disagree with the rect they
    /// are blitted into for one frame.
    ///
    /// `.max(1)` on both dimensions (plan gotcha 13): the fuzzer drives
    /// `Resize` down to a 1x2 frame, and a 0-width/0-height viewport would
    /// reach `Document::set_width`'s wrap engine with a wrap column of `0`.
    pub fn relayout(&mut self) {
        if self.frame_width == 0 || self.frame_height == 0 {
            return;
        }
        let area = ratatui::layout::Rect::new(0, 0, self.frame_width, self.frame_height);
        let geo = crate::layout::geometry(area, self);
        let (w, h) = (geo.editor.width.max(1), geo.editor.height.max(1));
        self.active_doc_mut().viewport.set_size(w, h);
    }

    /// Re-runs the display pipeline for the ACTIVE document and caches the
    /// result on it for `render::draw` to blit. Safe to call more than once
    /// per message batch — see `Document::sync`'s docs. Only the active
    /// document is synced (Phase 1/WP1: exactly one document is ever
    /// visible) — a later multi-pane WP re-evaluates this against whichever
    /// documents are actually on screen.
    ///
    /// The messages pane's log document is re-synced FIRST, before
    /// `relayout`: `relayout` sizes the editor viewport from a rect that has
    /// the pane's height carved out of it, and that height is derived from
    /// this very sync. Syncing afterwards would leave `relayout` reading a
    /// height computed for the previous frame's width and log contents, so the
    /// editor's viewport would trail the pane by one pass — visible as rows
    /// built for a viewport taller or shorter than the rect they land in, and
    /// as a page-scroll that pages by a screenful that is not on screen. Both
    /// the sync and this ordering stay in the settle step, never inside
    /// `render::draw`, which only ever borrows the app.
    ///
    /// Derives the active document's `focused` flag from `App::focus` after
    /// that, every call: a Guard being up means the editor is never really
    /// focused, while the messages pane deliberately is NOT part of that gate
    /// because it is non-modal — a message arriving must not blur the editor.
    /// The search bar is a second input with its own caret
    /// (`render::search::draw`); the document is never ALSO `focused` while
    /// it's open, or the editor caret would keep painting under a bar that
    /// is actually eating every keystroke. Reveal is pushed as its own flag
    /// (`reveal_engaged`) WITHOUT the search-bar gate: the bar's match
    /// navigation drives the document cursor, and a jump into a concealed
    /// element must reveal it even though the caret stays blurred.
    pub fn sync_view(&mut self) {
        let width = self.frame_width;
        let frame_height = self.frame_height;
        crate::messages::sync(self, width, frame_height);
        self.relayout();
        let engaged = self.focus() == Pane::Editor && self.guard.is_none();
        let focused = engaged && self.search.is_none();
        self.active_doc_mut().focused = focused;
        self.active_doc_mut().reveal_engaged = engaged;
        // Plan WP5.S2: mirrors `App::icons` (the one startup-decided tier)
        // onto the active document, same "outside writer pushes an
        // App-held decision down before every sync" shape as `focused`
        // right above — `Document` itself holds no `App` reference to read
        // this from.
        self.active_doc_mut().icons = self.icons.clone();
        let view = self.active_doc_mut().sync();
        self.active_doc_mut().view = Some(view);
        // A no-op with the bar closed; with it open, recomputes the match
        // set when the active document or its buffer version has drifted
        // since the last recompute (a tab switch, an undo/redo, an
        // external reload) — every draft edit already triggers its own
        // recompute directly from `search::keys::handle_key`.
        crate::search::sync(self);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::app::App;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.frame_width = 80;
        app.frame_height = 24;
        app
    }

    /// The document is never `focused` while the search bar is open — the
    /// bar paints its own caret (`render::search::draw`), so the editor's
    /// caret must stop claiming focus too, or both would paint at once.
    #[test]
    fn the_document_loses_focus_while_the_search_bar_is_open() {
        let mut app = app();
        app.sync_view();
        assert!(app.active_doc().focused, "editor is focused before ^F");

        crate::search::open(&mut app);
        app.sync_view();

        assert!(
            !app.active_doc().focused,
            "the document must not paint a caret while the bar is open"
        );
    }
}
