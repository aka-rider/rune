use std::sync::Arc;

use rune_image::CellSize;

use crate::app::App;
use crate::document::DocumentId;
use crate::graphics::ImageStatus;
use crate::runtime::Effects;

pub(crate) fn refit_on_resize(app: &mut App, effects: &mut Effects) {
    let id = app.active;
    let Some(doc) = app.doc(id) else { return };
    let pane_width = doc.viewport.width as usize;
    let cell = app.graphics.cell;
    let kitty = app.graphics.kitty;

    if doc.image().is_some() {
        refit_whole_document(app, id, pane_width, cell, kitty, effects);
    } else if doc.embeds().is_some() {
        refit_embeds(app, id, pane_width, cell, kitty, effects);
    }
}

fn refit_whole_document(
    app: &mut App,
    id: DocumentId,
    pane_width: usize,
    cell: CellSize,
    kitty: bool,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc(id) else { return };
    let Some(image) = doc.image() else { return };
    let ImageStatus::Live {
        cells: current_cells,
        ..
    } = &image.status
    else {
        return;
    };
    let Some(decoded_px) = image.dims else { return };
    let fit = super::footprint::fit(decoded_px, pane_width, cell);
    let cells = fit.cells;
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
    if kitty {
        let generation = image.next_generation.mint();
        image.in_flight = Some(generation);
        effects.cmds.push(super::decode_cmd::encode_image_cmd(
            id, decoded, img_id, cells, cell, generation, true,
        ));
    }
    if fit.truncated {
        crate::messages::warn(
            app,
            "image is taller than this terminal can address \u{2014} bottom rows are cropped",
        );
    }
}

fn refit_embeds(
    app: &mut App,
    id: DocumentId,
    pane_width: usize,
    cell: CellSize,
    kitty: bool,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc(id) else { return };
    let Some(embeds) = doc.embeds() else { return };
    let changed: Vec<String> = embeds
        .images
        .iter()
        .filter_map(|(target, state)| {
            let ImageStatus::Live {
                cells: current_cells,
                ..
            } = &state.status
            else {
                return None;
            };
            let decoded_px = state.dims?;
            let fit = super::footprint::fit(decoded_px, pane_width, cell);
            (fit.cells != *current_cells).then(|| target.clone())
        })
        .collect();

    let mut any_truncated = false;
    for target in changed {
        let Some(doc) = app.doc_mut(id) else { return };
        let Some(embeds) = doc.embeds_mut() else {
            return;
        };
        let Some(state) = embeds.images.get_mut(&target) else {
            continue;
        };
        let Some(decoded_px) = state.dims else {
            continue;
        };
        let fit = super::footprint::fit(decoded_px, pane_width, cell);
        any_truncated |= fit.truncated;
        let cells = fit.cells;
        let img_id = state.id;
        let ImageStatus::Live { decoded, .. } =
            std::mem::replace(&mut state.status, ImageStatus::Pending)
        else {
            continue;
        };
        state.status = ImageStatus::Live {
            decoded: Arc::clone(&decoded),
            cells,
        };
        if kitty {
            embeds.next_generation = embeds.next_generation.wrapping_add(1);
            let generation = embeds.next_generation;
            let Some(state) = embeds.images.get_mut(&target) else {
                continue;
            };
            state.in_flight = Some(generation);
            effects.cmds.push(super::embed::encode_embed_cmd(
                id, decoded, img_id, cells, cell, generation,
            ));
        }
    }
    if any_truncated {
        crate::messages::warn(
            app,
            "an embedded image is taller than this terminal can address \u{2014} bottom rows are cropped",
        );
    }
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
                    crate::graphics::handle_image_decoded(
                        app,
                        doc,
                        generation,
                        result,
                        &mut effects,
                    );
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
        crate::graphics::handle_image_encoded(
            &mut app,
            doc,
            generation,
            was_live,
            result,
            &mut stale_reply,
        );

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
