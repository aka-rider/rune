use crate::app::App;
use crate::focus::{self, FocusTarget};
use crate::pane::Pane;

const MIN_EDITOR_CELLS: u16 = 1;

impl App {
    pub(crate) fn editor_viewport_size(&self) -> (u16, u16) {
        let geo = crate::layout::geometry(self.frame_area(), self);
        (
            geo.editor.width.max(MIN_EDITOR_CELLS),
            geo.editor.height.max(MIN_EDITOR_CELLS),
        )
    }

    pub fn relayout(&mut self) {
        if self.frame.is_none() {
            return;
        }
        let geo = crate::layout::geometry(self.frame_area(), self);
        let (w, h) = self.editor_viewport_size();
        self.active_doc_mut().viewport.set_size(w, h);
        if let Some(preview) = self.explorer.preview.as_mut() {
            preview.doc.viewport.set_size(w, h);
        }
        if let Some(diff) = self.diff.as_mut() {
            let (lw, lh) = geo.diff_left.map_or((w, h), |diff_left| {
                (diff_left.width.max(1), diff_left.height.max(1))
            });
            diff.left.viewport.set_size(lw, lh);
        }
    }

    pub fn sync_view(&mut self) {
        let width = self.frame_width();
        let frame_height = self.frame_height();
        // Must run before `relayout`: `relayout` sizes the editor viewport
        // from a rect with this pane's height carved out, and that height
        // comes from this sync. Syncing after would leave `relayout` reading
        // last frame's pane height, so the editor viewport would trail the
        // pane by one pass.
        crate::messages::sync(self, width, frame_height);
        self.relayout();
        let engaged = self.focus() == Pane::Editor && self.guard.is_none();
        let target = focus::target(self);
        let focused = (target == FocusTarget::Editor
            || (target == FocusTarget::Palette && self.focus() == Pane::Editor))
            && self.guard.is_none()
            && self.find().is_none();
        self.active_doc_mut().focused = focused;
        // Not gated on the find panel like `focused` above: the panel's match
        // navigation drives the document cursor, and a jump into a
        // concealed element must reveal it even though the caret stays
        // blurred.
        self.active_doc_mut().reveal_engaged = engaged;
        // `Document` holds no reference to `App`, so `icon_tier` is pushed
        // down onto it here rather than read directly.
        let icons = self.icons();
        self.active_doc_mut().icons = icons;
        // Runs before the document sync below so a live match's own byte
        // offsets can reach the reveal decision that sync makes — a match
        // sitting inside concealed markup must reveal it, the same way the
        // caret already does.
        crate::find::sync(self);
        let search_offsets = crate::find::reveal_offsets(self);
        self.active_doc_mut()
            .set_search_reveal_offsets(search_offsets);
        let view = self.active_doc_mut().sync();
        self.active_doc_mut().view = Some(view);
        self.sync_preview();
        crate::diff_view::sync(self);
    }

    fn sync_preview(&mut self) {
        let icons = self.icons();
        let Some(preview) = self.explorer.preview.as_mut() else {
            return;
        };
        preview.doc.icons = icons;
        let view = preview.doc.sync_without_caret();
        preview.doc.view = Some(view);
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
        app.frame = Some(crate::app::FrameSize::new(80, 24));
        app
    }

    #[test]
    fn the_document_loses_focus_while_the_find_panel_is_open() {
        let mut app = app();
        app.sync_view();
        assert!(app.active_doc().focused, "editor is focused before ^F");

        crate::find::open(&mut app, false, &mut crate::runtime::Effects::default());
        app.sync_view();

        assert!(
            !app.active_doc().focused,
            "the document must not paint a caret while the panel is open"
        );
    }
}
