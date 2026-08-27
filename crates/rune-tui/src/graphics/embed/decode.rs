use std::sync::Arc;

use crate::app::App;
use crate::document::DocumentId;
use crate::graphics::ImageStatus;
use crate::runtime::{Cmd, CmdError, Effects, Msg};

pub(crate) fn schedule_embed_decode(
    app: &mut App,
    id: DocumentId,
    target: &str,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc_mut(id) else { return };
    let Some(embeds) = doc.embeds_mut() else {
        return;
    };
    let already_in_flight = embeds
        .images
        .get(target)
        .is_some_and(|s| s.in_flight.is_some());
    if already_in_flight {
        return;
    }
    embeds.next_generation = embeds.next_generation.wrapping_add(1);
    let generation = embeds.next_generation;
    let Some(state) = embeds.images.get_mut(target) else {
        return;
    };
    state.in_flight = Some(generation);
    let path = state.abs_path.clone();
    let vfs = Arc::clone(&app.vfs);
    effects
        .cmds
        .push(super::super::decode_cmd::decode_embed_cmd(
            id, vfs, path, generation,
        ));
}

pub(crate) fn handle_embed_decoded(
    app: &mut App,
    id: DocumentId,
    generation: u64,
    result: Result<rune_image::decode::Decoded, CmdError>,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc(id) else { return };
    let Some(target) = doc.embeds().and_then(|embeds| {
        embeds
            .images
            .iter()
            .find(|(_, s)| s.in_flight == Some(generation))
            .map(|(k, _)| k.clone())
    }) else {
        return;
    };
    let pane_width = doc.viewport.width as usize;
    let cell = app.graphics.cell;
    let kitty = app.graphics.kitty;

    let Some(doc) = app.doc_mut(id) else { return };
    let Some(embeds) = doc.embeds_mut() else {
        return;
    };
    let Some(state) = embeds.images.get_mut(&target) else {
        return;
    };

    let decoded = match result {
        Ok(decoded) => decoded,
        Err(e) => {
            state.in_flight = None;
            state.status = ImageStatus::Failed(e.to_string());
            return;
        }
    };

    state.dims = Some(rune_image::PixelSize {
        w: decoded.width,
        h: decoded.height,
    });
    let fit = crate::graphics::footprint::fit(
        rune_image::PixelSize {
            w: decoded.width,
            h: decoded.height,
        },
        pane_width,
        cell,
    );
    let cells = fit.cells;
    let img_id = state.id;
    let decoded = Arc::new(decoded);

    if !kitty {
        state.in_flight = None;
        state.status = ImageStatus::Live { decoded, cells };
    } else {
        embeds.next_generation = embeds.next_generation.wrapping_add(1);
        let generation = embeds.next_generation;
        let Some(state) = embeds.images.get_mut(&target) else {
            return;
        };
        state.in_flight = Some(generation);
        state.status = ImageStatus::Live {
            decoded: Arc::clone(&decoded),
            cells,
        };
        effects.cmds.push(encode_embed_cmd(
            id, decoded, img_id, cells, cell, generation,
        ));
    }
    if fit.truncated {
        crate::messages::warn(
            app,
            "an embedded image is taller than this terminal can address \u{2014} bottom rows are cropped",
        );
    }
}

pub(crate) fn handle_embed_encoded(
    app: &mut App,
    id: DocumentId,
    generation: u64,
    result: Result<rune_image::Transmit, CmdError>,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc_mut(id) else { return };
    let Some(embeds) = doc.embeds_mut() else {
        return;
    };
    let Some((_, state)) = embeds
        .images
        .iter_mut()
        .find(|(_, s)| s.in_flight == Some(generation))
    else {
        return;
    };
    state.in_flight = None;
    match result {
        Ok(transmit) => effects.transmit(transmit),
        Err(e) => state.status = ImageStatus::Failed(e.to_string()),
    }
}

pub(crate) fn encode_embed_cmd(
    doc: DocumentId,
    decoded: Arc<rune_image::decode::Decoded>,
    img_id: rune_image::ImageId,
    cells: rune_image::CellFootprint,
    cell: rune_image::CellSize,
    generation: u64,
) -> Cmd {
    Cmd::image_encode(move || {
        let result =
            rune_image::fit_and_encode(&decoded, img_id.get(), cells.cols, cells.rows, cell)
                .map_err(CmdError::from);
        Some(Msg::EmbedEncoded {
            doc,
            generation,
            result,
        })
    })
}
