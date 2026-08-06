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
    pub fn sync_view(&mut self) {
        let width = self.frame_width;
        let frame_height = self.frame_height;
        crate::messages::sync(self, width, frame_height);
        self.relayout();
        let focused = self.focus() == Pane::Editor && self.guard.is_none();
        self.active_doc_mut().focused = focused;
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
