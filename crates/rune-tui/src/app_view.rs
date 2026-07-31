//! `App::relayout`/`App::sync_view` — the geometry-and-display settle step
//! every message batch runs before the next draw (split out of `app.rs`,
//! §1.6 budget). Still plain `impl App` methods; nothing about how they are
//! reached from the rest of the crate changes.

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
    /// Called as the first statement of `sync_view` below (the runtime
    /// calls `sync_view` immediately before every `render::draw`, so that's
    /// the one chokepoint no call site can forget) AND
    /// again from `Msg::Resize` itself, so tests that call `update` without
    /// a following `sync_view` still see a correctly-sized viewport;
    /// calling it twice in the same message batch is harmless (it's a pure
    /// function of `frame_width`/`frame_height`, idempotent either way).
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
    /// Derives the active document's `focused` flag from `App::focus` FIRST,
    /// every call (plan Gotchas: `&& app.modal.is_none()` — a modal up means
    /// the editor is never really focused). Also re-syncs the modal
    /// document, if one is up (WP3.S3), at the terminal's own width — kept
    /// in this settle step, never inside `render::draw` itself (§5.4).
    pub fn sync_view(&mut self) {
        self.relayout();
        let focused = self.focus() == Pane::Editor && self.modal.is_none();
        self.active_doc_mut().focused = focused;
        // Plan WP5.S2: mirrors `App::icons` (the one startup-decided tier)
        // onto the active document, same "outside writer pushes an
        // App-held decision down before every sync" shape as `focused`
        // right above — `Document` itself holds no `App` reference to read
        // this from.
        self.active_doc_mut().icons = self.icons.clone();
        let view = self.active_doc_mut().sync();
        self.active_doc_mut().view = Some(view);
        if self.modal.is_some() {
            let width = self.frame_width;
            let frame_height = self.frame_height;
            crate::banner::sync_modal(self, width, frame_height);
        }
    }
}
