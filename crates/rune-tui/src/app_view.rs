use crate::app::App;
use crate::focus::{self, FocusTarget};
use crate::pane::Pane;

impl App {
    pub fn relayout(&mut self) {
        if self.frame.is_none() {
            return;
        }
        let area = self.frame_area();
        let geo = crate::layout::geometry(area, self);
        // Floors both dimensions at 1: the fuzzer drives `Resize` down to a
        // 1x2 frame, and a 0-width/0-height viewport would reach
        // `Document::set_width`'s wrap engine with a wrap column of 0.
        let (w, h) = (geo.editor.width.max(1), geo.editor.height.max(1));
        self.active_doc_mut().viewport.set_size(w, h);
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
            && self.search().is_none();
        self.active_doc_mut().focused = focused;
        // Not gated on the search bar like `focused` above: the bar's match
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
        crate::search::sync(self);
        let active = self.active;
        let search_offsets = self
            .search()
            .filter(|state| state.doc == active)
            .map(|state| {
                state
                    .matches
                    .iter()
                    .flat_map(|m| [m.start, m.end.saturating_sub(1).max(m.start)])
                    .collect()
            })
            .unwrap_or_default();
        self.active_doc_mut()
            .set_search_reveal_offsets(search_offsets);
        let view = self.active_doc_mut().sync();
        self.active_doc_mut().view = Some(view);
        crate::diff_view::sync(self);
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
    fn the_document_loses_focus_while_the_search_bar_is_open() {
        let mut app = app();
        app.sync_view();
        assert!(app.active_doc().focused, "editor is focused before ^F");

        crate::search::open(&mut app, &mut crate::runtime::Effects::default());
        app.sync_view();

        assert!(
            !app.active_doc().focused,
            "the document must not paint a caret while the bar is open"
        );
    }
}
