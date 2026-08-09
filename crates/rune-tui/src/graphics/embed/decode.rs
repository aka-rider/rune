//! One embed's decode `Cmd` and reply handler (plan WP9), the `EmbedSet`
//! sibling of `graphics::decode_cmd`'s whole-image-document pair. Reuses
//! the EXISTING `Msg::ImageDecoded { doc, generation, result }` variant
//! rather than adding a new one — `runtime/mod.rs` already exceeds the
//! 500-line ceiling from unrelated concurrent work and must not grow further
//! for this package. `generation` alone is enough to find the right
//! `EmbedState`: `EmbedSet::next_generation` mints a value unique across
//! EVERY embed in one document (not just within one), so
//! `handle_embed_decoded` can recover the target key by scanning for
//! `in_flight == Some(generation)` without the `Msg` itself naming it.
//! `dispatch::update_inner`'s `Msg::ImageDecoded` arm is what routes a
//! reply here instead of to `graphics::handle_image_decoded`: a document
//! is exclusively an image DOCUMENT (`doc.image.is_some()`) or a markdown
//! document that may hold embeds, never both, so there is no ambiguity to
//! resolve at that fork.

use std::sync::Arc;

use crate::app::App;
use crate::document::DocumentId;
use crate::graphics::ImageStatus;
use crate::runtime::Effects;

/// Spawns a decode for the embed named `target` in document `id`, iff one
/// isn't already in flight for it. A no-op if the target isn't tracked at
/// all (the reconciler always inserts the `EmbedState` before calling this,
/// but a defensive miss here is cheaper than a panic).
pub(crate) fn schedule_embed_decode(
    app: &mut App,
    id: DocumentId,
    target: &str,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc_mut(id) else { return };
    let already_in_flight = doc
        .embeds
        .images
        .get(target)
        .is_some_and(|s| s.in_flight.is_some());
    if already_in_flight {
        return;
    }
    doc.embeds.next_generation = doc.embeds.next_generation.wrapping_add(1);
    let generation = doc.embeds.next_generation;
    let Some(state) = doc.embeds.images.get_mut(target) else {
        return;
    };
    state.in_flight = Some(generation);
    let path = state.abs_path.clone();
    let vfs = Arc::clone(&app.vfs);
    effects
        .cmds
        .push(super::super::decode_cmd::decode_image_cmd(
            id, vfs, path, generation,
        ));
}

/// Applies a `Msg::ImageDecoded` reply that belongs to an EMBED rather than
/// a whole image document (plan WP9, mirroring `graphics::decode_cmd::
/// handle_image_decoded`'s own fixed order exactly): find the target this
/// `generation` belongs to, drop silently if none matches (a stale reply,
/// or the embed was despawned while its decode was in flight); otherwise
/// clear `in_flight`, record failure or compute the fit-to-width footprint
/// and — Kitty only — push a transmit escape.
pub(crate) fn handle_embed_decoded(
    app: &mut App,
    id: DocumentId,
    generation: u64,
    result: Result<rune_image::decode::Decoded, String>,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc(id) else { return };
    let Some(target) = doc
        .embeds
        .images
        .iter()
        .find(|(_, s)| s.in_flight == Some(generation))
        .map(|(k, _)| k.clone())
    else {
        return;
    };
    let pane_width = doc.viewport.width as usize;
    let cell = app.graphics.cell;
    let kitty = app.graphics.kitty;

    let Some(doc) = app.doc_mut(id) else { return };
    let Some(state) = doc.embeds.images.get_mut(&target) else {
        return;
    };
    state.in_flight = None;

    let decoded = match result {
        Ok(decoded) => decoded,
        Err(e) => {
            state.status = ImageStatus::Failed(e);
            return;
        }
    };

    state.dims = Some((decoded.width, decoded.height));
    let (cols, rows) =
        crate::graphics::footprint::fit(decoded.width, decoded.height, pane_width, cell);
    state.cells = Some((cols, rows));
    let img_id = state.id;
    let raw = kitty
        .then(|| rune_image::fit_and_encode(&decoded, img_id, cols, rows, cell).ok())
        .flatten();
    state.decoded = Some(decoded);
    state.status = ImageStatus::Live;
    if let Some(bytes) = raw {
        effects.raw.push(bytes.into_bytes());
    }
}
