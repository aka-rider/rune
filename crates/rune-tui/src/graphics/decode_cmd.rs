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
    let generation = image.in_flight.unwrap_or(0).wrapping_add(1);
    let path = image.path.clone();
    let vfs = Arc::clone(&app.vfs);

    let Some(doc) = app.doc_mut(id) else { return };
    let Some(image) = doc.image.as_mut() else {
        return;
    };
    image.in_flight = Some(generation);
    effects
        .cmds
        .push(decode_image_cmd(id, vfs, path, generation));
}

/// Reads `path` off-thread via `vfs.read` (§1.4.9) and decodes it — the
/// off-thread half of an image document's lifecycle, §5.4: decode is CPU
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
/// encodes and pushes a transmit escape into `effects.raw`.
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
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::path::Path;

    use rune_core::buffer::Buffer;
    use rune_image::CellSize;
    use rune_vfs::Mem;

    use super::*;

    const X_PNG: &[u8] = include_bytes!("../../../../golang/testdata/assets/x.png");

    fn app_with_pending_image(kitty: bool) -> (App, DocumentId) {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/vault/x.png"), X_PNG)
            .expect("seed x.png");
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new(""), None, vfs, None);
        app.graphics.kitty = kitty;
        app.graphics.cell = CellSize { w: 8, h: 16 };
        let id =
            crate::workspace::open_path(&mut app, Path::new("/vault/x.png")).expect("open x.png");
        app.doc_mut(id).expect("doc").viewport.set_size(80, 24);
        (app, id)
    }

    fn decode_x_png() -> rune_image::decode::Decoded {
        rune_image::decode_still(X_PNG).expect("decode x.png")
    }

    #[test]
    fn scheduling_a_decode_marks_in_flight_and_pushes_one_cmd() {
        let (mut app, id) = app_with_pending_image(true);
        let mut effects = Effects::default();
        schedule_image_decode(&mut app, id, &mut effects);
        assert_eq!(effects.cmds.len(), 1);
        assert_eq!(effects.cmds[0].kind(), CmdKind::ImageDecode);
        assert!(
            app.doc(id)
                .unwrap()
                .image
                .as_ref()
                .unwrap()
                .in_flight
                .is_some()
        );
    }

    #[test]
    fn scheduling_twice_only_ever_spawns_one_cmd() {
        let (mut app, id) = app_with_pending_image(true);
        let mut effects = Effects::default();
        schedule_image_decode(&mut app, id, &mut effects);
        schedule_image_decode(&mut app, id, &mut effects);
        assert_eq!(effects.cmds.len(), 1, "already in flight — no second Cmd");
    }

    #[test]
    fn a_successful_decode_goes_live_and_transmits_when_kitty_is_on() {
        let (mut app, id) = app_with_pending_image(true);
        app.doc_mut(id)
            .expect("doc")
            .image
            .as_mut()
            .unwrap()
            .in_flight = Some(1);
        let mut effects = Effects::default();
        handle_image_decoded(&mut app, id, 1, Ok(decode_x_png()), &mut effects);

        let image = app.doc(id).unwrap().image.as_ref().unwrap();
        assert_eq!(image.status, ImageStatus::Live);
        assert!(image.in_flight.is_none());
        assert!(image.cells.is_some());
        assert_eq!(effects.raw.len(), 1);
        assert!(effects.raw[0].starts_with(b"\x1b_G"));
    }

    #[test]
    fn a_successful_decode_never_transmits_when_kitty_is_off() {
        let (mut app, id) = app_with_pending_image(false);
        app.doc_mut(id)
            .expect("doc")
            .image
            .as_mut()
            .unwrap()
            .in_flight = Some(1);
        let mut effects = Effects::default();
        handle_image_decoded(&mut app, id, 1, Ok(decode_x_png()), &mut effects);

        assert!(effects.raw.is_empty());
        assert_eq!(
            app.doc(id).unwrap().image.as_ref().unwrap().status,
            ImageStatus::Live,
            "the fit/footprint is still computed even without Kitty"
        );
    }

    #[test]
    fn a_failed_decode_becomes_failed_status_with_no_raw_output() {
        let (mut app, id) = app_with_pending_image(true);
        app.doc_mut(id)
            .expect("doc")
            .image
            .as_mut()
            .unwrap()
            .in_flight = Some(1);
        let mut effects = Effects::default();
        handle_image_decoded(&mut app, id, 1, Err("boom".to_string()), &mut effects);

        let image = app.doc(id).unwrap().image.as_ref().unwrap();
        assert!(matches!(&image.status, ImageStatus::Failed(msg) if msg == "boom"));
        assert!(effects.raw.is_empty());
    }

    #[test]
    fn a_stale_generation_is_dropped_with_no_effects() {
        let (mut app, id) = app_with_pending_image(true);
        app.doc_mut(id)
            .expect("doc")
            .image
            .as_mut()
            .unwrap()
            .in_flight = Some(2);
        let mut effects = Effects::default();
        // generation 1 no longer matches the live in_flight of 2.
        handle_image_decoded(&mut app, id, 1, Ok(decode_x_png()), &mut effects);

        let image = app.doc(id).unwrap().image.as_ref().unwrap();
        assert_eq!(
            image.status,
            ImageStatus::Pending,
            "stale reply must not apply"
        );
        assert_eq!(
            image.in_flight,
            Some(2),
            "stale reply must not clear in_flight"
        );
        assert!(effects.raw.is_empty());
    }
}
