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
/// no-op for any non-image document, so the editor table's `⌘R` binding
/// (gated on the `image` `when` atom for UX purposes only) can never do
/// anything harmful even if that gate were bypassed. `ImageState::id` is
/// never reallocated across this call, so the eventual transmit (`handle_
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
        effects.force_redraw = true;
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

    /// Drives a pending image document all the way to `Live` through the
    /// real `schedule_image_decode` -> `Cmd::run` -> `handle_image_decoded`
    /// path (plan WP6.S1's "reload" tests want an already-open, already-
    /// transmitted image to reload from, not a hand-constructed one).
    fn app_with_live_image(kitty: bool) -> (App, DocumentId) {
        let (mut app, id) = app_with_pending_image(kitty);
        let mut effects = Effects::default();
        schedule_image_decode(&mut app, id, &mut effects);
        for cmd in effects.cmds {
            if let Some(Msg::ImageDecoded {
                doc,
                generation,
                result,
            }) = cmd.run()
            {
                handle_image_decoded(&mut app, doc, generation, result, &mut Effects::default());
            }
        }
        (app, id)
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

    #[test]
    fn reload_retransmits_under_the_same_id_and_forces_a_redraw() {
        let (mut app, id) = app_with_live_image(true);
        let original_id = app.doc(id).unwrap().image.as_ref().unwrap().id;

        let mut effects = Effects::default();
        reload_image(&mut app, id, &mut effects);
        assert_eq!(effects.cmds.len(), 1, "reload must spawn exactly one Cmd");
        for cmd in effects.cmds {
            if let Some(Msg::ImageDecoded {
                doc,
                generation,
                result,
            }) = cmd.run()
            {
                let mut reply_effects = Effects::default();
                handle_image_decoded(&mut app, doc, generation, result, &mut reply_effects);
                assert_eq!(reply_effects.raw.len(), 1);
                assert!(reply_effects.raw[0].starts_with(b"\x1b_G"));
                assert!(reply_effects.force_redraw, "a reload must force a redraw");
            }
        }

        let reloaded_id = app.doc(id).unwrap().image.as_ref().unwrap().id;
        assert_eq!(
            reloaded_id, original_id,
            "reload must retransmit under the same deterministic id"
        );
        assert_eq!(
            app.doc(id).unwrap().image.as_ref().unwrap().status,
            ImageStatus::Live
        );
    }

    #[test]
    fn reload_recovers_a_failed_image_document() {
        let (mut app, id) = app_with_pending_image(true);
        app.doc_mut(id)
            .expect("doc")
            .image
            .as_mut()
            .unwrap()
            .in_flight = Some(1);
        handle_image_decoded(
            &mut app,
            id,
            1,
            Err("boom".to_string()),
            &mut Effects::default(),
        );
        assert!(matches!(
            app.doc(id).unwrap().image.as_ref().unwrap().status,
            ImageStatus::Failed(_)
        ));

        let mut effects = Effects::default();
        reload_image(&mut app, id, &mut effects);
        for cmd in effects.cmds {
            if let Some(Msg::ImageDecoded {
                doc,
                generation,
                result,
            }) = cmd.run()
            {
                handle_image_decoded(&mut app, doc, generation, result, &mut Effects::default());
            }
        }
        assert_eq!(
            app.doc(id).unwrap().image.as_ref().unwrap().status,
            ImageStatus::Live,
            "reload must recover a Failed image document"
        );
    }

    #[test]
    fn reload_is_a_no_op_on_a_non_image_document() {
        let mem = Arc::new(Mem::new());
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        let id = app.active;
        assert!(app.doc(id).unwrap().image.is_none());

        let mut effects = Effects::default();
        reload_image(&mut app, id, &mut effects);
        assert!(effects.cmds.is_empty());
        assert!(effects.raw.is_empty());
    }

    /// WP2.S2: reload used to refuse outright while `in_flight.is_some()`,
    /// which is exactly what let a lost reply wedge a document forever —
    /// the recovery command refused to run precisely when recovery was
    /// needed. It must now preempt: spawn a fresh decode anyway, abandoning
    /// whatever was in flight.
    #[test]
    fn reload_preempts_an_in_flight_decode_instead_of_refusing() {
        let (mut app, id) = app_with_live_image(true);
        app.doc_mut(id)
            .expect("doc")
            .image
            .as_mut()
            .unwrap()
            .in_flight = Some(99);

        let mut effects = Effects::default();
        reload_image(&mut app, id, &mut effects);
        assert_eq!(
            effects.cmds.len(),
            1,
            "in flight or not, reload must always spawn a fresh decode"
        );
    }

    /// WP2.S2: the abandoned decode's eventual reply (stamped with the OLD
    /// generation) must be dropped without disturbing the document, once
    /// the fresh decode from a preempting reload has already landed.
    #[test]
    fn a_reply_abandoned_by_a_preempting_reload_is_dropped() {
        let (mut app, id) = app_with_live_image(true);
        app.doc_mut(id)
            .expect("doc")
            .image
            .as_mut()
            .unwrap()
            .in_flight = Some(1);

        let mut effects = Effects::default();
        reload_image(&mut app, id, &mut effects);
        let new_generation = app.doc(id).unwrap().image.as_ref().unwrap().in_flight;
        assert_eq!(
            new_generation,
            Some(2),
            "reload must mint a strictly greater generation than the abandoned one"
        );

        // The abandoned decode's reply finally lands, still carrying the
        // OLD generation.
        let mut stale_effects = Effects::default();
        handle_image_decoded(&mut app, id, 1, Ok(decode_x_png()), &mut stale_effects);
        assert!(stale_effects.raw.is_empty(), "stale reply must not act");
        assert_eq!(
            app.doc(id).unwrap().image.as_ref().unwrap().in_flight,
            Some(2),
            "the stale reply must not clear the fresh decode's in_flight"
        );
    }

    /// WP2.S1's "Done when": two successive reloads must never collapse to
    /// the same generation — the bug this plan fixes was `spawn_decode`
    /// deriving the generation from `in_flight.unwrap_or(0)`, which is
    /// always exactly `1` from every caller that has already proven
    /// `in_flight.is_none()`.
    #[test]
    fn two_successive_reloads_produce_different_generations() {
        let (mut app, id) = app_with_live_image(true);

        let mut first_effects = Effects::default();
        reload_image(&mut app, id, &mut first_effects);
        let first_generation = app.doc(id).unwrap().image.as_ref().unwrap().in_flight;

        // Land the first reload's reply before issuing the second reload,
        // so the second is a genuinely fresh (non-preempting) spawn too.
        for cmd in first_effects.cmds {
            if let Some(Msg::ImageDecoded {
                doc,
                generation,
                result,
            }) = cmd.run()
            {
                handle_image_decoded(&mut app, doc, generation, result, &mut Effects::default());
            }
        }

        let mut second_effects = Effects::default();
        reload_image(&mut app, id, &mut second_effects);
        let second_generation = app.doc(id).unwrap().image.as_ref().unwrap().in_flight;

        assert_ne!(
            first_generation, second_generation,
            "each reload must mint a strictly new generation"
        );
    }
}
