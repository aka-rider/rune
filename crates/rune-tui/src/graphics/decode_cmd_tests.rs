//! Tests for the image-document decode lifecycle — split out to keep the
//! parent under the file-size ceiling, the same shape `resolve_tests.rs`
//! and `layout_tests.rs` already use elsewhere in the workspace.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use rune_core::buffer::Buffer;
use rune_image::CellSize;
use rune_vfs::Mem;

use crate::runtime::CmdKind;

use super::*;

const X_PNG: &[u8] = include_bytes!("../../../../testdata/assets/x.png");

fn mint_gen(raw: u64) -> crate::generation::Generation {
    crate::generation::Generation::from_raw(raw)
}

fn app_with_pending_image(kitty: bool) -> (App, DocumentId) {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/vault/x.png"), X_PNG)
        .expect("seed x.png");
    let vfs: Arc<dyn Vfs + Send + Sync> = mem;
    let mut app = App::new(Buffer::new(""), None, vfs, None);
    app.graphics.kitty = kitty;
    app.graphics.cell = CellSize { w: 8, h: 16 };
    let id = crate::workspace::open_path(&mut app, Path::new("/vault/x.png")).expect("open x.png");
    app.doc_mut(id).expect("doc").viewport.set_size(80, 24);
    (app, id)
}

fn decode_x_png() -> rune_image::decode::Decoded {
    rune_image::decode_still(X_PNG).expect("decode x.png")
}

fn is_live(app: &App, id: DocumentId) -> bool {
    matches!(
        app.doc(id).unwrap().image().unwrap().status,
        ImageStatus::Live { .. }
    )
}

/// Drives a pending image document all the way to `Live` through the
/// real `schedule_image_decode` -> `Cmd::run` -> `handle_image_decoded`
/// path, so reload tests get an already-open, already-transmitted image
/// to reload from, not a hand-constructed one.
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
    assert!(app.doc(id).unwrap().image().unwrap().in_flight.is_some());
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
    app.doc_mut(id).expect("doc").image_mut().unwrap().in_flight = Some(mint_gen(1));
    let mut effects = Effects::default();
    handle_image_decoded(&mut app, id, mint_gen(1), Ok(decode_x_png()), &mut effects);

    let image = app.doc(id).unwrap().image().unwrap();
    assert!(matches!(image.status, ImageStatus::Live { .. }));
    assert!(image.in_flight.is_none());
    assert_eq!(effects.transmits().len(), 1);
    assert!(effects.transmits()[0].chunks()[0].starts_with(b"\x1b_G"));
}

#[test]
fn a_successful_decode_never_transmits_when_kitty_is_off() {
    let (mut app, id) = app_with_pending_image(false);
    app.doc_mut(id).expect("doc").image_mut().unwrap().in_flight = Some(mint_gen(1));
    let mut effects = Effects::default();
    handle_image_decoded(&mut app, id, mint_gen(1), Ok(decode_x_png()), &mut effects);

    assert!(effects.raw_bytes().is_empty());
    assert!(
        is_live(&app, id),
        "the fit/footprint is still computed even without Kitty"
    );
}

#[test]
fn a_failed_decode_becomes_failed_status_with_no_raw_output() {
    let (mut app, id) = app_with_pending_image(true);
    app.doc_mut(id).expect("doc").image_mut().unwrap().in_flight = Some(mint_gen(1));
    let mut effects = Effects::default();
    handle_image_decoded(
        &mut app,
        id,
        mint_gen(1),
        Err(CmdError::Refused("boom".to_string())),
        &mut effects,
    );

    let image = app.doc(id).unwrap().image().unwrap();
    assert!(matches!(&image.status, ImageStatus::Failed(msg) if msg == "boom"));
    assert!(effects.raw_bytes().is_empty());
}

#[test]
fn a_stale_generation_is_dropped_with_no_effects() {
    let (mut app, id) = app_with_pending_image(true);
    app.doc_mut(id).expect("doc").image_mut().unwrap().in_flight = Some(mint_gen(2));
    let mut effects = Effects::default();
    // generation 1 no longer matches the live in_flight of 2.
    handle_image_decoded(&mut app, id, mint_gen(1), Ok(decode_x_png()), &mut effects);

    let image = app.doc(id).unwrap().image().unwrap();
    assert!(
        matches!(image.status, ImageStatus::Pending),
        "stale reply must not apply"
    );
    assert_eq!(
        image.in_flight,
        Some(mint_gen(2)),
        "stale reply must not clear in_flight"
    );
    assert!(effects.raw_bytes().is_empty());
}

/// A FIRST transmit must not ask for a forced redraw. `force_redraw` clears
/// the terminal, and a clear issued from a decode reply was observed to block
/// the main thread indefinitely — a hang needing an external kill. It is only
/// ever needed for a retransmit, whose placeholder cells can be byte-identical
/// to what is already on screen while the pixels behind them changed. A first
/// transmit replaces the info card with placeholder cells, which the renderer's
/// own diff already sees.
#[test]
fn a_first_transmit_does_not_force_a_redraw() {
    let (mut app, id) = app_with_pending_image(true);

    let mut effects = Effects::default();
    schedule_image_decode(&mut app, id, &mut effects);
    for cmd in effects.cmds {
        if let Some(Msg::ImageDecoded {
            doc,
            generation,
            result,
        }) = cmd.run()
        {
            let mut reply = Effects::default();
            handle_image_decoded(&mut app, doc, generation, result, &mut reply);
            assert_eq!(
                reply.transmits().len(),
                1,
                "the image must still be transmitted"
            );
            assert!(
                !reply.force_redraw,
                "a first transmit must not clear the terminal"
            );
        }
    }
    assert!(is_live(&app, id));
}

#[test]
fn reload_retransmits_under_the_same_id_and_forces_a_redraw() {
    let (mut app, id) = app_with_live_image(true);
    let original_id = app.doc(id).unwrap().image().unwrap().id;

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
            assert_eq!(reply_effects.transmits().len(), 1);
            assert!(reply_effects.transmits()[0].chunks()[0].starts_with(b"\x1b_G"));
            assert!(reply_effects.force_redraw, "a reload must force a redraw");
        }
    }

    let reloaded_id = app.doc(id).unwrap().image().unwrap().id;
    assert_eq!(
        reloaded_id, original_id,
        "reload must retransmit under the same deterministic id"
    );
    assert!(is_live(&app, id));
}

#[test]
fn reload_recovers_a_failed_image_document() {
    let (mut app, id) = app_with_pending_image(true);
    app.doc_mut(id).expect("doc").image_mut().unwrap().in_flight = Some(mint_gen(1));
    handle_image_decoded(
        &mut app,
        id,
        mint_gen(1),
        Err(CmdError::Refused("boom".to_string())),
        &mut Effects::default(),
    );
    assert!(matches!(
        app.doc(id).unwrap().image().unwrap().status,
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
    assert!(
        is_live(&app, id),
        "reload must recover a Failed image document"
    );
}

#[test]
fn reload_is_a_no_op_on_a_non_image_document() {
    let mem = Arc::new(Mem::new());
    let vfs: Arc<dyn Vfs + Send + Sync> = mem;
    let mut app = App::new(Buffer::new("hello"), None, vfs, None);
    let id = app.active;
    assert!(app.doc(id).unwrap().image().is_none());

    let mut effects = Effects::default();
    reload_image(&mut app, id, &mut effects);
    assert!(effects.cmds.is_empty());
    assert!(effects.raw_bytes().is_empty());
}

/// Reload must preempt rather than refuse while `in_flight.is_some()`:
/// refusing there is exactly what let a lost reply wedge a document
/// forever, since the recovery command refused to run precisely when
/// recovery was needed. It spawns a fresh decode anyway, abandoning
/// whatever was in flight.
#[test]
fn reload_preempts_an_in_flight_decode_instead_of_refusing() {
    let (mut app, id) = app_with_live_image(true);
    app.doc_mut(id).expect("doc").image_mut().unwrap().in_flight = Some(mint_gen(99));

    let mut effects = Effects::default();
    reload_image(&mut app, id, &mut effects);
    assert_eq!(
        effects.cmds.len(),
        1,
        "in flight or not, reload must always spawn a fresh decode"
    );
}

/// The abandoned decode's eventual reply (stamped with the OLD
/// generation) must be dropped without disturbing the document, once
/// the fresh decode from a preempting reload has already landed.
#[test]
fn a_reply_abandoned_by_a_preempting_reload_is_dropped() {
    let (mut app, id) = app_with_live_image(true);
    // `app_with_live_image` already minted once (returning 0, its own
    // real `in_flight` since cleared) — `mint_gen(0)` stands in for that
    // abandoned decode's actual generation.
    app.doc_mut(id).expect("doc").image_mut().unwrap().in_flight = Some(mint_gen(0));

    let mut effects = Effects::default();
    reload_image(&mut app, id, &mut effects);
    let new_generation = app.doc(id).unwrap().image().unwrap().in_flight;
    assert_eq!(
        new_generation,
        Some(mint_gen(1)),
        "reload must mint a strictly greater generation than the abandoned one"
    );

    // The abandoned decode's reply finally lands, still carrying the
    // OLD generation.
    let mut stale_effects = Effects::default();
    handle_image_decoded(
        &mut app,
        id,
        mint_gen(0),
        Ok(decode_x_png()),
        &mut stale_effects,
    );
    assert!(
        stale_effects.raw_bytes().is_empty(),
        "stale reply must not act"
    );
    assert_eq!(
        app.doc(id).unwrap().image().unwrap().in_flight,
        Some(mint_gen(1)),
        "the stale reply must not clear the fresh decode's in_flight"
    );
}

/// Two successive reloads must never collapse to the same generation —
/// a prior bug had `spawn_decode` deriving the generation from
/// `in_flight.unwrap_or(0)`, which is always exactly `1` from every
/// caller that has already proven `in_flight.is_none()`.
#[test]
fn two_successive_reloads_produce_different_generations() {
    let (mut app, id) = app_with_live_image(true);

    let mut first_effects = Effects::default();
    reload_image(&mut app, id, &mut first_effects);
    let first_generation = app.doc(id).unwrap().image().unwrap().in_flight;

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
    let second_generation = app.doc(id).unwrap().image().unwrap().in_flight;

    assert_ne!(
        first_generation, second_generation,
        "each reload must mint a strictly new generation"
    );
}
