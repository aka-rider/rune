//! Pinning marks a tab the upcoming tab-cap eviction must never pick. The
//! toggle refuses previews because a preview is a transient the user never
//! opened.

use crate::app::App;
use crate::document::DocumentId;

/// Flips the active document's pin, refusing (with its own warn message) on
/// a preview tab.
pub fn toggle_pin(app: &mut App, id: DocumentId) {
    if app.refuse_if_preview(id) {
        return;
    }
    if let Some(doc) = app.doc_mut(id) {
        doc.pinned = !doc.pinned;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::document::ReadOnly;
    use crate::messages;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.active_doc_mut().viewport.set_size(80, 23);
        app
    }

    fn active_row(app: &App, width: u16) -> String {
        let buf = crate::testgrid::draw_with(width, 4, |frame| {
            crate::opentabs::draw(app, ratatui::layout::Rect::new(0, 0, width, 4), frame)
        });
        (0..width)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()))
            .collect()
    }

    #[test]
    fn toggle_pin_flips_the_flag_and_draw_shows_the_marker() {
        let mut app = app();
        let active = app.active;

        toggle_pin(&mut app, active);
        assert!(app.doc(active).unwrap().pinned);
        assert_eq!(&active_row(&app, 20)[0..5], "  1:*");

        toggle_pin(&mut app, active);
        assert!(!app.doc(active).unwrap().pinned);
        assert_eq!(&active_row(&app, 20)[0..5], "  1: ");
    }

    #[test]
    fn toggle_pin_refuses_a_preview() {
        let mut app = app();
        let active = app.active;
        app.doc_mut(active).unwrap().read_only = ReadOnly::Preview;

        toggle_pin(&mut app, active);

        assert!(!app.doc(active).unwrap().pinned);
        assert_eq!(
            messages::newest_text(&app),
            ReadOnly::Preview.refusal_message()
        );
    }
}
