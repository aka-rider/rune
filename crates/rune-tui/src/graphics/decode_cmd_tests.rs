#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use rune_core::buffer::Buffer;
use rune_image::CellSize;
use rune_vfs::{Mem, VfsTestExt};

use crate::runtime::CmdKind;

use super::*;

const X_PNG: &[u8] = include_bytes!("../../../../testdata/assets/x.png");

fn mint_gen(raw: u64) -> crate::generation::ImageDecodeGen {
    crate::generation::ImageDecodeGen::from_raw(raw)
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

fn app_with_live_image(kitty: bool) -> (App, DocumentId) {
    let (mut app, id) = app_with_pending_image(kitty);
    let mut effects = Effects::default();
    schedule_image_decode(&mut app, id, &mut effects);
    settle_cmds(&mut app, effects.cmds);
    (app, id)
}

fn run_decoded_then_encoded(
    app: &mut App,
    doc: DocumentId,
    generation: crate::generation::ImageDecodeGen,
    result: Result<rune_image::decode::Decoded, CmdError>,
) -> Effects {
    let mut decode_effects = Effects::default();
    handle_image_decoded(app, doc, generation, result, &mut decode_effects);
    let mut encode_effects = Effects::default();
    for cmd in decode_effects.cmds {
        if let Some(Msg::ImageEncoded {
            doc,
            generation,
            was_live,
            result,
        }) = cmd.run()
        {
            handle_image_encoded(app, doc, generation, was_live, result, &mut encode_effects);
        }
    }
    encode_effects
}

fn settle_cmds(app: &mut App, cmds: Vec<crate::runtime::Cmd>) {
    for cmd in cmds {
        match cmd.run() {
            Some(Msg::ImageDecoded {
                doc,
                generation,
                result,
            }) => {
                let mut effects = Effects::default();
                handle_image_decoded(app, doc, generation, result, &mut effects);
                settle_cmds(app, effects.cmds);
            }
            Some(Msg::ImageEncoded {
                doc,
                generation,
                was_live,
                result,
            }) => {
                handle_image_encoded(
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
fn a_successful_decode_spawns_an_encode_cmd_and_stays_in_flight() {
    let (mut app, id) = app_with_pending_image(true);
    app.doc_mut(id).expect("doc").image_mut().unwrap().in_flight = Some(mint_gen(1));
    let mut effects = Effects::default();
    handle_image_decoded(&mut app, id, mint_gen(1), Ok(decode_x_png()), &mut effects);

    let image = app.doc(id).unwrap().image().unwrap();
    assert!(matches!(image.status, ImageStatus::Live { .. }));
    assert!(
        image.in_flight.is_some(),
        "the encode is now the outstanding async op"
    );
    assert!(effects.transmits().is_empty(), "no transmit until encode replies");
    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::ImageEncode);
}

#[test]
fn a_successful_decode_goes_live_and_transmits_when_kitty_is_on() {
    let (mut app, id) = app_with_pending_image(true);
    app.doc_mut(id).expect("doc").image_mut().unwrap().in_flight = Some(mint_gen(1));
    let effects = run_decoded_then_encoded(&mut app, id, mint_gen(1), Ok(decode_x_png()));

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
            let reply = run_decoded_then_encoded(&mut app, doc, generation, result);
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
            let reply_effects = run_decoded_then_encoded(&mut app, doc, generation, result);
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

#[test]
fn a_reply_abandoned_by_a_preempting_reload_is_dropped() {
    let (mut app, id) = app_with_live_image(true);
    app.doc_mut(id).expect("doc").image_mut().unwrap().in_flight = Some(mint_gen(0));

    let mut effects = Effects::default();
    reload_image(&mut app, id, &mut effects);
    let new_generation = app.doc(id).unwrap().image().unwrap().in_flight;
    assert_eq!(
        new_generation,
        Some(mint_gen(2)),
        "reload must mint a strictly greater generation than the abandoned one"
    );

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
        Some(mint_gen(2)),
        "the stale reply must not clear the fresh decode's in_flight"
    );
}

#[test]
fn two_successive_reloads_produce_different_generations() {
    let (mut app, id) = app_with_live_image(true);

    let mut first_effects = Effects::default();
    reload_image(&mut app, id, &mut first_effects);
    let first_generation = app.doc(id).unwrap().image().unwrap().in_flight;

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

#[test]
fn a_stale_encode_reply_is_dropped_without_disturbing_the_live_image() {
    let (mut app, id) = app_with_live_image(true);
    let ImageStatus::Live {
        cells: live_cells, ..
    } = &app.doc(id).unwrap().image().unwrap().status
    else {
        unreachable!("test setup: image must already be Live");
    };
    let live_cells = *live_cells;

    let mut effects = Effects::default();
    reload_image(&mut app, id, &mut effects);
    let fresh_generation = app.doc(id).unwrap().image().unwrap().in_flight;
    assert!(fresh_generation.is_some(), "test setup: reload must be in flight");

    let mut stale_effects = Effects::default();
    handle_image_encoded(
        &mut app,
        id,
        mint_gen(0),
        true,
        Ok(rune_image::fit_and_encode(&decode_x_png(), 1, live_cells.cols, live_cells.rows, CellSize { w: 8, h: 16 })
            .expect("encode")),
        &mut stale_effects,
    );

    assert!(
        stale_effects.transmits().is_empty(),
        "a stale encode reply must not transmit"
    );
    assert!(
        !stale_effects.force_redraw,
        "a stale encode reply must not force a redraw"
    );
    assert_eq!(
        app.doc(id).unwrap().image().unwrap().in_flight,
        fresh_generation,
        "a stale encode reply must not clear the fresh reload's in_flight"
    );
}
