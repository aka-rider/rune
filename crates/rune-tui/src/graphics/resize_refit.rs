//! Re-fitting a live image document's footprint on `Msg::Resize`:
//! a pane resize (or a cell-geometry re-derivation with no pane
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

use std::sync::Arc;

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
    let Some(image) = doc.image() else { return };
    let ImageStatus::Live {
        cells: current_cells,
        ..
    } = &image.status
    else {
        return;
    };
    let Some(decoded_px) = image.dims else { return };
    let cells = super::footprint::fit(decoded_px, pane_width, cell);
    if cells == *current_cells {
        return;
    }
    let img_id = image.id;

    let Some(doc) = app.doc_mut(id) else { return };
    let Some(image) = doc.image_mut() else {
        return;
    };
    let ImageStatus::Live { decoded, .. } =
        std::mem::replace(&mut image.status, ImageStatus::Pending)
    else {
        return;
    };
    image.status = ImageStatus::Live {
        decoded: Arc::clone(&decoded),
        cells,
    };
    if !kitty {
        return;
    }
    let generation = image.next_generation.mint();
    image.in_flight = Some(generation);
    effects.cmds.push(super::decode_cmd::encode_image_cmd(
        id, decoded, img_id, cells, cell, generation, true,
    ));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_image::CellSize;
    use rune_vfs::{Mem, Vfs, VfsTestExt};

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
        settle_cmds(&mut app, effects.cmds);
        (app, id)
    }

    fn settle_cmds(app: &mut App, cmds: Vec<crate::runtime::Cmd>) {
        for cmd in cmds {
            match cmd.run() {
                Some(crate::runtime::Msg::ImageDecoded {
                    doc,
                    generation,
                    result,
                }) => {
                    let mut effects = Effects::default();
                    crate::graphics::handle_image_decoded(app, doc, generation, result, &mut effects);
                    settle_cmds(app, effects.cmds);
                }
                Some(crate::runtime::Msg::ImageEncoded {
                    doc,
                    generation,
                    was_live,
                    result,
                }) => {
                    crate::graphics::handle_image_encoded(
                        app,
                        doc,
                        generation,
                        was_live,
                        result,
                        &mut Effects::default(),
                    );
                }
                _ => {}
            }
        }
    }

    fn live_cells(app: &App, id: DocumentId) -> Option<rune_image::CellFootprint> {
        match &app.doc(id).unwrap().image().unwrap().status {
            ImageStatus::Live { cells, .. } => Some(*cells),
            _ => None,
        }
    }

    #[test]
    fn a_footprint_change_retransmits_and_forces_a_redraw() {
        let (mut app, id) = app_with_live_image();
        let before = live_cells(&app, id);
        // Narrow the pane so the fit-to-width footprint must shrink.
        app.doc_mut(id).expect("doc").viewport.set_size(4, 24);

        let mut effects = Effects::default();
        refit_on_resize(&mut app, &mut effects);
        assert_eq!(effects.cmds.len(), 1, "the re-encode must run off-thread");

        let after = live_cells(&app, id);
        assert_ne!(before, after, "the footprint must actually have changed");

        let mut reply_effects = Effects::default();
        if let Some(crate::runtime::Msg::ImageEncoded {
            doc,
            generation,
            was_live,
            result,
        }) = effects.cmds.remove(0).run()
        {
            crate::graphics::handle_image_encoded(
                &mut app,
                doc,
                generation,
                was_live,
                result,
                &mut reply_effects,
            );
        }
        assert_eq!(reply_effects.transmits().len(), 1);
        assert!(reply_effects.transmits()[0].chunks()[0].starts_with(b"\x1b_G"));
        assert!(reply_effects.force_redraw);
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

        assert!(effects.transmits().is_empty());
        assert!(!effects.force_redraw);
    }

    #[test]
    fn a_resize_storm_abandons_the_earlier_still_in_flight_encode() {
        let (mut app, id) = app_with_live_image();
        app.doc_mut(id).expect("doc").viewport.set_size(4, 24);
        let mut first = Effects::default();
        refit_on_resize(&mut app, &mut first);
        assert_eq!(first.cmds.len(), 1);
        let stale_generation = app.doc(id).expect("doc").image().expect("image").in_flight;

        app.doc_mut(id).expect("doc").viewport.set_size(80, 24);
        let mut second = Effects::default();
        refit_on_resize(&mut app, &mut second);
        assert_eq!(second.cmds.len(), 1);
        let fresh_generation = app.doc(id).expect("doc").image().expect("image").in_flight;
        assert_ne!(stale_generation, fresh_generation);

        let Some(crate::runtime::Msg::ImageEncoded {
            doc,
            generation,
            was_live,
            result,
        }) = first.cmds.remove(0).run()
        else {
            unreachable!("expected an ImageEncoded reply");
        };
        let mut stale_reply = Effects::default();
        crate::graphics::handle_image_encoded(&mut app, doc, generation, was_live, result, &mut stale_reply);

        assert!(
            stale_reply.transmits().is_empty(),
            "the abandoned resize's encode must not transmit"
        );
        assert!(!stale_reply.force_redraw);
        assert_eq!(
            app.doc(id).expect("doc").image().expect("image").in_flight,
            fresh_generation,
            "the stale reply must not clear the fresh resize's in_flight"
        );
    }
}
