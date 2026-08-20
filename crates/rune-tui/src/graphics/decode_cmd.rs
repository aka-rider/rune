use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_image::{CellFootprint, CellSize, ImageId};

use rune_vfs::Vfs;

use crate::app::App;
use crate::document::DocumentId;
use crate::graphics::ImageStatus;
use crate::runtime::{Cmd, CmdError, Effects, Msg};

pub(crate) fn schedule_image_decode(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let Some(image) = doc.image() else { return };
    if !matches!(image.status, ImageStatus::Pending) || image.in_flight.is_some() {
        return;
    }
    spawn_decode(app, id, effects);
}

pub(crate) fn reload_image(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    if doc.image().is_none() {
        return;
    }
    spawn_decode(app, id, effects);
}

fn spawn_decode(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let Some(image) = doc.image() else { return };
    let path = image.path.clone();
    let vfs = Arc::clone(&app.vfs);

    let Some(doc) = app.doc_mut(id) else { return };
    let Some(image) = doc.image_mut() else {
        return;
    };
    let generation = image.next_generation.mint();
    image.in_flight = Some(generation);
    effects
        .cmds
        .push(decode_image_cmd(id, vfs, path, generation));
}

fn read_and_decode(vfs: &dyn Vfs, path: &Path) -> Result<rune_image::decode::Decoded, CmdError> {
    let sighting = rune_vfs::get(vfs, path, None)?;
    Ok(rune_image::decode_still(&sighting.bytes)?)
}

pub(super) fn decode_image_cmd(
    doc: DocumentId,
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    generation: crate::generation::ImageDecodeGen,
) -> Cmd {
    Cmd::image_decode(move || {
        let result = read_and_decode(vfs.as_ref(), &path);
        Some(Msg::ImageDecoded {
            doc,
            generation,
            result,
        })
    })
}

pub(super) fn decode_embed_cmd(
    doc: DocumentId,
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    generation: u64,
) -> Cmd {
    Cmd::image_decode(move || {
        let result = read_and_decode(vfs.as_ref(), &path);
        Some(Msg::EmbedDecoded {
            doc,
            generation,
            result,
        })
    })
}

pub(super) fn encode_image_cmd(
    doc: DocumentId,
    decoded: Arc<rune_image::decode::Decoded>,
    img_id: ImageId,
    cells: CellFootprint,
    cell: CellSize,
    generation: crate::generation::ImageDecodeGen,
    was_live: bool,
) -> Cmd {
    Cmd::image_encode(move || {
        let result =
            rune_image::fit_and_encode(&decoded, img_id.get(), cells.cols, cells.rows, cell)
                .map_err(CmdError::from);
        Some(Msg::ImageEncoded {
            doc,
            generation,
            was_live,
            result,
        })
    })
}

pub(crate) fn handle_image_decoded(
    app: &mut App,
    id: DocumentId,
    generation: crate::generation::ImageDecodeGen,
    result: Result<rune_image::decode::Decoded, CmdError>,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc(id) else { return };
    let Some(image) = doc.image() else { return };
    if image.in_flight != Some(generation) {
        return;
    }
    let pane_width = doc.viewport.width as usize;
    let cell = app.graphics.cell;
    let kitty = app.graphics.kitty;
    let img_id = image.id;
    let was_live = matches!(image.status, ImageStatus::Live { .. });

    let Some(doc) = app.doc_mut(id) else { return };
    let Some(image) = doc.image_mut() else {
        return;
    };

    let decoded = match result {
        Ok(decoded) => decoded,
        Err(e) => {
            image.in_flight = None;
            image.status = ImageStatus::Failed(e.to_string());
            return;
        }
    };

    image.dims = Some(rune_image::PixelSize {
        w: decoded.width,
        h: decoded.height,
    });
    let cells = super::footprint::fit(
        rune_image::PixelSize {
            w: decoded.width,
            h: decoded.height,
        },
        pane_width,
        cell,
    );
    let decoded = Arc::new(decoded);

    if !kitty {
        image.in_flight = None;
        image.status = ImageStatus::Live { decoded, cells };
        return;
    }

    let generation = image.next_generation.mint();
    image.in_flight = Some(generation);
    image.status = ImageStatus::Live {
        decoded: Arc::clone(&decoded),
        cells,
    };
    effects.cmds.push(encode_image_cmd(
        id, decoded, img_id, cells, cell, generation, was_live,
    ));
}

pub(crate) fn handle_image_encoded(
    app: &mut App,
    id: DocumentId,
    generation: crate::generation::ImageDecodeGen,
    was_live: bool,
    result: Result<rune_image::Transmit, CmdError>,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc_mut(id) else { return };
    let Some(image) = doc.image_mut() else {
        return;
    };
    if image.in_flight != Some(generation) {
        return;
    }
    image.in_flight = None;
    match result {
        Ok(transmit) => {
            effects.transmit(transmit);
            effects.force_redraw |= was_live;
        }
        Err(e) => {
            image.status = ImageStatus::Failed(e.to_string());
        }
    }
}

#[cfg(test)]
#[path = "decode_cmd_tests.rs"]
mod tests;
