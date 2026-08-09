//! Re-fitting a live image document's footprint on `Msg::Resize` (plan
//! WP5.S6): a pane resize (or a cell-geometry re-derivation with no pane
//! resize at all — a Retina-aware terminal reporting new pixel dimensions
//! at the SAME cols/rows) can change the fit-to-width `(cols, rows)` an
//! already-decoded image occupies. Retransmitting is only worth doing when
//! that footprint actually changed — re-encoding and re-writing identical
//! escape bytes on every keystroke-adjacent resize would be wasted work —
//! but whenever it does, ratatui's own diffing cannot be trusted to notice:
//! the placeholder cells stay byte-identical (same id, same diacritics)
//! across a footprint that only changed which cells' PIXELS the terminal
//! shows, so the retransmit is paired with a forced full redraw
//! (`Effects::force_redraw`) or the terminal would keep showing the stale
//! placement.

use crate::app::App;
use crate::graphics::ImageStatus;
use crate::runtime::Effects;

/// Called from `dispatch::update_inner`'s `Msg::Resize` arm, AFTER
/// `App::relayout` has already sized the active document's `Viewport` —
/// the footprint math needs that pane width to be current. A no-op unless
/// the active document is a `Live` image document AND the recomputed
/// `(cols, rows)` actually differs from what's already reserved.
///
/// `app.graphics.cell` itself, in contrast, is NOT yet current for the
/// resize event that triggered this call: `runtime::apply` re-derives it
/// from the terminal's own reported pixel dimensions only AFTER the whole
/// message (including this dispatch) has run. A resize that changes cell
/// PIXEL geometry (not column/row count — a Retina-aware terminal
/// reporting different pixel dimensions at the same cols/rows) therefore
/// lags by one resize event before this re-fit sees the new geometry —
/// self-correcting on the very next `Msg::Resize`, which is frequent
/// during an interactive drag-resize and otherwise harmless (the pane's
/// COLUMN width, the dominant factor in fit-to-width, is already current).
/// Reordering `runtime::apply` to detect graphics before dispatching is
/// out of this package's scope.
pub(crate) fn refit_on_resize(app: &mut App, effects: &mut Effects) {
    let id = app.active;
    let Some(doc) = app.doc(id) else { return };
    let pane_width = doc.viewport.width as usize;
    let cell = app.graphics.cell;
    let kitty = app.graphics.kitty;
    let Some(image) = &doc.image else { return };
    if image.status != ImageStatus::Live {
        return;
    }
    let Some(decoded_dims) = image.decoded.as_ref().map(|d| (d.width, d.height)) else {
        return;
    };
    let (cols, rows) = super::footprint::fit(decoded_dims.0, decoded_dims.1, pane_width, cell);
    if Some((cols, rows)) == image.cells {
        return;
    }
    let img_id = image.id;

    let raw = kitty
        .then(|| {
            doc.image
                .as_ref()
                .and_then(|i| i.decoded.as_ref())
                .and_then(|decoded| {
                    rune_image::fit_and_encode(decoded, img_id, cols, rows, cell).ok()
                })
        })
        .flatten();

    let Some(doc) = app.doc_mut(id) else { return };
    let Some(image) = doc.image.as_mut() else {
        return;
    };
    image.cells = Some((cols, rows));

    if let Some(bytes) = raw {
        effects.raw.push(bytes.into_bytes());
        effects.force_redraw = true;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_image::CellSize;
    use rune_vfs::{Mem, Vfs};

    use super::*;
    use crate::document::DocumentId;

    const X_PNG: &[u8] = include_bytes!("../../../../testdata/assets/x.png");

    fn app_with_live_image() -> (App, DocumentId) {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/vault/x.png"), X_PNG)
            .expect("seed x.png");
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new(""), None, vfs, None);
        app.graphics.kitty = true;
        app.graphics.cell = CellSize { w: 8, h: 16 };
        let id =
            crate::workspace::open_path(&mut app, Path::new("/vault/x.png")).expect("open x.png");
        app.doc_mut(id).expect("doc").viewport.set_size(80, 24);
        let mut effects = Effects::default();
        crate::graphics::schedule_image_decode(&mut app, id, &mut effects);
        for cmd in effects.cmds {
            if let Some(crate::runtime::Msg::ImageDecoded {
                doc,
                generation,
                result,
            }) = cmd.run()
            {
                crate::graphics::handle_image_decoded(
                    &mut app,
                    doc,
                    generation,
                    result,
                    &mut Effects::default(),
                );
            }
        }
        (app, id)
    }

    #[test]
    fn a_footprint_change_retransmits_and_forces_a_redraw() {
        let (mut app, id) = app_with_live_image();
        let before = app.doc(id).unwrap().image.as_ref().unwrap().cells;
        // Narrow the pane so the fit-to-width footprint must shrink.
        app.doc_mut(id).expect("doc").viewport.set_size(4, 24);

        let mut effects = Effects::default();
        refit_on_resize(&mut app, &mut effects);

        let after = app.doc(id).unwrap().image.as_ref().unwrap().cells;
        assert_ne!(before, after, "the footprint must actually have changed");
        assert_eq!(effects.raw.len(), 1);
        assert!(effects.raw[0].starts_with(b"\x1b_G"));
        assert!(effects.force_redraw);
    }

    #[test]
    fn an_unchanged_footprint_neither_retransmits_nor_redraws() {
        let (mut app, id) = app_with_live_image();
        let _ = id;
        // The viewport is still exactly the size it was when the image
        // went live — the footprint is already correct, so no resize
        // happened at all from this function's point of view.

        let mut effects = Effects::default();
        refit_on_resize(&mut app, &mut effects);

        assert!(effects.raw.is_empty());
        assert!(!effects.force_redraw);
    }
}
