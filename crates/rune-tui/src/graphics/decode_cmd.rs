//! The image document's decode `Cmd` (plan WP5.S1), its scheduling
//! chokepoint, and the reply handler that turns a finished decode into a
//! live footprint plus (Kitty only) a transmit escape (WP5.S2/S3).
//!
//! Mirrors `highlight::schedule_highlight`'s shape deliberately: an
//! `in_flight` generation guards against a second decode for the same
//! document racing the first, and a reply whose generation no longer
//! matches is dropped silently with no further `Cmd` — `spawn_cmd` has no
//! cancellation, so this echo is the only thing standing between a stale
//! reply and a corrupted `ImageState`.
//!
//! The fit computation (`footprint::fit`) deliberately runs in the REPLY
//! handler, not in the `Cmd` closure: a document can become active (and
//! this decode spawned) before its own `Viewport` has ever been sized by
//! `App::relayout` — the CLI's synchronous bootstrap open in particular has
//! no `Msg::Resize` behind it yet at spawn time. By the time an async
//! reply lands, at least one `sync_view`/`relayout` has already run for
//! this document, so its `viewport.width` is trustworthy.

use std::path::PathBuf;
use std::sync::Arc;

use rune_vfs::Vfs;

use crate::app::App;
use crate::document::DocumentId;
use crate::graphics::ImageStatus;
use crate::runtime::{Cmd, CmdKind, Effects, Msg};

/// Spawns a decode for `id` if — and only if — it is an image document
/// still waiting on its very first decode: `status == Pending` (a prior
/// success moved it to `Live`, a prior failure to `Failed`, and WP5 adds no
/// retry path of its own — that's WP6's reload command) and no decode is
/// already `in_flight` for it. A no-op for every other document, so every
/// call site (the `App::update` active-change hook, `runtime::bootstrap`'s
/// startup kick) can call this unconditionally without checking `kind`
/// itself first.
pub(crate) fn schedule_image_decode(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let Some(image) = &doc.image else { return };
    if image.status != ImageStatus::Pending || image.in_flight.is_some() {
        return;
    }
    spawn_decode(app, id, effects);
}

/// The reload command (plan WP6.S1, made preempting by WP2.S2): re-reads
/// and re-decodes an already-open image document on demand, through the
/// very same `Vfs`/decode `Cmd` `schedule_image_decode` uses — the only
/// difference is this function does NOT check `status`, since reload's
/// whole point is recovering a `Failed` document or refreshing an already-
/// `Live` one, not just filling in a `Pending` one. Unlike `schedule_image_
/// decode`, it does NOT refuse when a decode is already `in_flight`: it
/// abandons it instead — `spawn_decode` mints a strictly greater generation
/// than any this document has issued before, so the abandoned reply is
/// later dropped by `handle_image_decoded`'s own `in_flight != Some(
/// generation)` guard rather than accepted as if it belonged to the fresh
/// decode. This is the recovery path for a reply that is ever lost (a
/// panicked decode thread, a failed channel send): with no timeout or
/// reaper anywhere in this pipeline, an explicit reload is the only way
/// out of a wedge, so it must always be able to spawn a new attempt. A
/// no-op for any non-image document, so the editor table's `⌘R` binding can
/// never do anything harmful even on a document `dispatch::Command::
/// Reload`'s own gate somehow let through. `ImageState::id` is never
/// reallocated across this call, so the eventual transmit (`handle_
/// image_decoded`) necessarily retransmits under the SAME deterministic id
/// the document opened with.
pub(crate) fn reload_image(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    if doc.image.is_none() {
        return;
    }
    spawn_decode(app, id, effects);
}

/// The shared spawn chokepoint both `schedule_image_decode` and `reload_
/// image` fall through to once their own gate has already passed: mints a
/// generation strictly greater than any this document has ever issued
/// (`ImageState::next_generation`, not derived from `in_flight` — WP2.S1:
/// `in_flight` goes back to `None` once a decode finishes or is abandoned,
/// so deriving from it would let a later spawn collide with an earlier,
/// still-outstanding one), snapshots the path and `Vfs` handle, marks
/// `in_flight`, and pushes the decode `Cmd`.
fn spawn_decode(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let Some(image) = &doc.image else { return };
    let generation = image.next_generation.wrapping_add(1);
    let path = image.path.clone();
    let vfs = Arc::clone(&app.vfs);

    let Some(doc) = app.doc_mut(id) else { return };
    let Some(image) = doc.image.as_mut() else {
        return;
    };
    image.next_generation = generation;
    image.in_flight = Some(generation);
    effects
        .cmds
        .push(decode_image_cmd(id, vfs, path, generation));
}

/// Reads `path` off-thread via `vfs.read` and decodes it — the
/// off-thread half of an image document's lifecycle: decode is CPU
/// work, and a large/degraded-filesystem image must never block the main
/// loop. No `catch_unwind` of its own: `spawn_cmd` already contains a
/// decoder panic on malformed input.
fn decode_image_cmd(
    doc: DocumentId,
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    generation: u64,
) -> Cmd {
    Cmd::new(CmdKind::ImageDecode, move || {
        let result = vfs
            .read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| rune_image::decode_still(&bytes).map_err(|e| e.to_string()));
        Some(Msg::ImageDecoded {
            doc,
            generation,
            result,
        })
    })
}

/// Applies a `Msg::ImageDecoded` reply (plan WP5.S2/S3). Fixed order: (a)
/// drop a stale generation with no further work at all — `in_flight` no
/// longer names it, so this reply describes a decode this document isn't
/// waiting on anymore; (b) otherwise clear `in_flight` unconditionally, so
/// a document can never wedge waiting on a decode that already returned;
/// (c) a decode error becomes `ImageStatus::Failed`, the info card's own
/// reason line; (d) success computes the fit-to-width footprint from the
/// CURRENT pane width and cell geometry, stores it (feeding the producer
/// via `Document::view`'s existing `image.cells` read), and — Kitty only —
/// encodes and pushes a transmit escape into `effects.raw`, forcing a full
/// redraw alongside it (plan WP6.S1/S6's reasoning: the reload command
/// reaches this exact handler, and a reload's placeholder cells are
/// typically byte-identical to what was already on screen — same id, same
/// diacritics — so ratatui's own "only repaint changed cells" diffing
/// cannot be trusted to notice the underlying pixels changed. Forcing it
/// unconditionally on every successful transmit, not just a reload, costs
/// nothing worse than one redundant `Terminal::clear` on the very first
/// open, which had nothing to redraw over anyway).
pub(crate) fn handle_image_decoded(
    app: &mut App,
    id: DocumentId,
    generation: u64,
    result: Result<rune_image::decode::Decoded, String>,
    effects: &mut Effects,
) {
    let Some(doc) = app.doc(id) else { return };
    let Some(image) = &doc.image else { return };
    if image.in_flight != Some(generation) {
        return;
    }
    let pane_width = doc.viewport.width as usize;
    let cell = app.graphics.cell;
    let kitty = app.graphics.kitty;
    let img_id = image.id;
    // Only a RETRANSMIT needs the diff invalidated: its placeholder cells can
    // be byte-identical to the ones already on screen while the pixels behind
    // them changed, so the renderer would skip them. A first transmit replaces
    // the info card with placeholder cells, which the diff sees on its own.
    let was_live = image.status == ImageStatus::Live;

    let Some(doc) = app.doc_mut(id) else { return };
    let Some(image) = doc.image.as_mut() else {
        return;
    };
    image.in_flight = None;

    let decoded = match result {
        Ok(decoded) => decoded,
        Err(e) => {
            image.status = ImageStatus::Failed(e);
            return;
        }
    };

    image.dims = Some((decoded.width, decoded.height));
    let (cols, rows) = super::footprint::fit(decoded.width, decoded.height, pane_width, cell);
    image.cells = Some((cols, rows));
    let raw = kitty
        .then(|| rune_image::fit_and_encode(&decoded, img_id, cols, rows, cell).ok())
        .flatten();
    image.decoded = Some(decoded);
    image.status = ImageStatus::Live;
    if let Some(bytes) = raw {
        effects.raw.push(bytes.into_bytes());
        effects.force_redraw |= was_live;
    }
}

#[cfg(test)]
#[path = "decode_cmd_tests.rs"]
mod tests;
