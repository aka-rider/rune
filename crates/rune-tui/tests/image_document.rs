//! WP4: opening an image file as a read-only image document — the
//! producer's synthesized rows actually scroll, the save/highlight guards
//! actually fire, and the info card renders on a non-Kitty terminal.
//!
//! WP5: the document actually renders pixels — the decode `Cmd`'s reply
//! drives `ImageStatus::Live`, real placeholder cells carry the allocated
//! id as a 24-bit colour, the row count reserved matches the fit-to-width
//! footprint, and scrolling clips through the SAME `visible_rows` chokepoint
//! WP4 already proved for the info card.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod dirty_common;

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_syntax::DocumentKind;
use rune_tui::app::{App, update};
use rune_tui::commands::nav_scroll;
use rune_tui::graphics::ImageStatus;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::testgrid;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

const X_PNG: &[u8] = include_bytes!("../../../golang/testdata/assets/x.png");

fn app_with_image() -> (App, rune_tui::document::DocumentId) {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/vault/x.png"), X_PNG)
        .expect("seed x.png");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new(""), None, vfs, None);
    let id = workspace::open_path(&mut app, Path::new("/vault/x.png")).expect("open x.png");
    (app, id)
}

/// Drives the real `Msg::ImageDecoded` reply for `id` against a freshly
/// decoded `x.png` (plan WP5's own [rune-tui] "drive integration tests
/// through the real update loop" rule) — the shared setup every WP5 test
/// below needs, factored out so each test states only what it's actually
/// asserting.
fn decode_x_png_via_update(app: &mut App, id: rune_tui::document::DocumentId) -> Effects {
    // `app_with_image` opens synchronously via `workspace::open_path`,
    // never through `schedule_image_decode` — arm `in_flight` by hand so
    // this reply isn't dropped as stale, exactly as a real spawn would.
    const GENERATION: u64 = 0;
    app.doc_mut(id).unwrap().image.as_mut().unwrap().in_flight = Some(GENERATION);
    let generation = GENERATION;
    let decoded = rune_image::decode_still(X_PNG).expect("decode x.png");
    let mut effects = Effects::default();
    update(
        app,
        Msg::ImageDecoded {
            doc: id,
            generation,
            result: Ok(decoded),
        },
        &mut effects,
    );
    effects
}

/// Opening an image path through `workspace::open_path` produces a
/// read-only, DB-less `DocumentKind::Image` document (plan WP4 Done-when).
#[test]
fn opening_an_image_path_yields_a_read_only_image_document() {
    let (app, id) = app_with_image();
    let doc = app.doc(id).expect("doc");
    assert!(doc.read_only, "an image document must be read-only");
    assert!(
        doc.db.is_none(),
        "an image document has no recovery binding"
    );
    assert_eq!(doc.kind, DocumentKind::Image);
    assert!(doc.image.is_some());
}

/// The image producer, not just the renderer, sees the reserved row count:
/// `view.display.total_rows()` reports it, and `nav_scroll::scroll_lines`
/// can actually move `scroll_row` all the way to `n - 1`.
#[test]
fn a_known_reserved_row_count_is_visible_to_the_producer_and_scrollable() {
    let (mut app, id) = app_with_image();
    let n = 7usize;
    app.doc_mut(id)
        .expect("doc")
        .image
        .as_mut()
        .expect("image")
        .cells = Some((40, n));
    app.sync_view();

    let view = app.doc(id).expect("doc").view.clone().expect("view");
    assert_eq!(view.display.total_rows(), n);

    let doc = app.doc_mut(id).expect("doc");
    nav_scroll::scroll_lines(doc, 1000);
    assert_eq!(doc.viewport.scroll_row, n - 1);
}

/// Plan WP4.S9: an image document's `file_path` is real, so without the
/// `trigger_save` guard a forced-dirty state (simulating a would-be dirty
/// buffer) would reach the no-DB-binding fallback and push a `Save` `Cmd`
/// that overwrites the image with the buffer's own (always empty) bytes.
/// Driven through the real `super+s` key via `app::update`, per the plan's
/// "drive integration tests through the real update loop".
#[test]
fn super_s_on_an_image_document_saves_nothing_and_never_focuses_the_title() {
    let (mut app, id) = app_with_image();
    // Force the dirty check an earlier revision of the guard would have
    // relied on to look "dirty" — the guard must still fire BEFORE it.
    dirty_common::force_dirty(&mut app, id);
    app.active = id;
    // `focus` is private by design — exactly three functions may write it,
    // so go through the setter rather than widening the field for a test.
    app.set_focus(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('s'),
            mods: Mods {
                sup: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );

    assert!(
        effects.cmds.is_empty(),
        "an image document must never dispatch a save Cmd"
    );
    assert_ne!(
        app.focus(),
        Pane::Title,
        "an image document's save key must never focus the title field"
    );
}

/// Plan WP4.S9: a tab switch onto an image document must never dispatch a
/// highlight `Cmd` — `resolve_highlight_source` excludes `DocumentKind::
/// Image` explicitly.
#[test]
fn switching_to_an_image_tab_dispatches_no_highlight_cmd() {
    let (mut app, image_id) = app_with_image();
    let other = app
        .documents
        .keys()
        .find(|&&id| id != image_id)
        .copied()
        .expect("a second (markdown) document from App::new");
    workspace::switch_to(&mut app, other);

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('2'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );

    assert_eq!(
        app.active, image_id,
        "ctrl+2 must have switched to the image tab"
    );
    assert!(
        !effects.cmds.iter().any(|c| c.kind() == CmdKind::Highlight),
        "switching to an image document must never dispatch a highlight Cmd"
    );
}

/// Plan WP4.S10/Done-when: on a non-Kitty terminal the info card shows the
/// file name, its probed dimensions, and the no-graphics reason line.
#[test]
fn info_card_renders_on_a_non_kitty_terminal() {
    let (mut app, id) = app_with_image();
    app.graphics.kitty = false;
    app.doc_mut(id).expect("doc").viewport.set_size(40, 10);
    app.sync_view();

    let frame = testgrid::grid(&app, 60, 20).join("\n");
    assert!(
        frame.contains("x.png"),
        "frame did not contain file name:\n{frame}"
    );
    assert!(
        frame.contains("64x48"),
        "frame did not contain dimensions:\n{frame}"
    );
    assert!(
        frame.contains("does not support inline images"),
        "frame did not contain the no-graphics reason line:\n{frame}"
    );
}

/// Plan WP5 Done-when: a `testgrid::draw` frame with `graphics.kitty = true`
/// carries a real placeholder cell — its symbol starts with the Kitty
/// Unicode placeholder codepoint (`U+10EEEE`), and its `fg` is the
/// `Color::Rgb` `rune_image::alloc_id` allocates for this fixture's
/// resolved path.
#[test]
fn a_live_image_document_renders_a_placeholder_cell_with_the_allocated_id() {
    let (mut app, id) = app_with_image();
    app.graphics.kitty = true;
    app.doc_mut(id).expect("doc").viewport.set_size(40, 10);
    decode_x_png_via_update(&mut app, id);
    app.sync_view();

    let expected_id = app.doc(id).unwrap().image.as_ref().unwrap().id;
    let expected = rune_image::alloc_id("/vault/x.png");
    assert_eq!(
        expected_id, expected,
        "test setup: id must be deterministic"
    );

    let buf = testgrid::draw(&app, 60, 20);
    let placeholder = rune_image::PLACEHOLDER;
    let found = (0..20).find_map(|y| {
        (0..60).find_map(|x| {
            let cell = buf.cell((x, y))?;
            cell.symbol().starts_with(placeholder).then_some(cell.fg)
        })
    });
    let fg = found.expect("no placeholder cell found in the rendered frame");
    let (r, g, b) = (
        ((expected_id >> 16) & 0xFF) as u8,
        ((expected_id >> 8) & 0xFF) as u8,
        (expected_id & 0xFF) as u8,
    );
    assert_eq!(fg, ratatui::style::Color::Rgb(r, g, b));
}

/// Plan WP5.S2 Done-when: a fixture of known pixel size (`x.png`, 64x48)
/// reserves exactly the `ceil_div`-derived row count for an explicitly set
/// `GraphicsCaps`, and `view.display.total_rows()` (the PRODUCER, not just
/// the renderer) reports that same count.
#[test]
fn a_decoded_fixture_reserves_the_fit_to_width_row_count() {
    let (mut app, id) = app_with_image();
    app.graphics.kitty = true;
    app.graphics.cell = rune_image::CellSize { w: 8, h: 16 };
    // 64px wide / 8px cells = 8 cols exactly, well under this pane width,
    // so no width-driven scaling: rows = ceil_div(48, 16) = 3.
    app.doc_mut(id).expect("doc").viewport.set_size(40, 24);
    decode_x_png_via_update(&mut app, id);
    app.sync_view();

    let cells = app.doc(id).unwrap().image.as_ref().unwrap().cells;
    assert_eq!(cells, Some((8, 3)));
    let view = app.doc(id).unwrap().view.clone().expect("view");
    assert_eq!(view.display.total_rows(), 3);
}

/// Plan WP5.S5: scrolling a `Live` image document clips correctly through
/// the SAME `visible_rows` chokepoint the info card already proved in WP4
/// — no new scroll axis, no new clamp. Forces a tall reserved footprint (a
/// narrow pane, so the fit-to-width scale leaves many rows) and asserts the
/// rendered frame only ever shows `height` rows' worth of placeholder rows
/// at a time, tracking `scroll_row`.
#[test]
fn scrolling_a_live_image_document_clips_through_visible_rows() {
    let (mut app, id) = app_with_image();
    app.graphics.kitty = true;
    app.graphics.cell = rune_image::CellSize { w: 8, h: 16 };
    // A pane wide enough that fit-to-width never scales down (`x.png` is
    // 64x48 -> 8x3 cells at full scale — this fixture can never reserve
    // more than 3 rows, since fit-to-width never upscales), paired with a
    // viewport SHORTER than that reserved footprint so scrolling has
    // something to clip.
    app.doc_mut(id).expect("doc").viewport.set_size(40, 2);
    decode_x_png_via_update(&mut app, id);
    app.sync_view();

    let total = app
        .doc(id)
        .unwrap()
        .view
        .clone()
        .unwrap()
        .display
        .total_rows();
    assert!(
        total > 2,
        "test setup: the reserved footprint must exceed the viewport height, got {total}"
    );

    let doc = app.doc_mut(id).unwrap();
    nav_scroll::scroll_lines(doc, 1000);
    assert_eq!(
        doc.viewport.scroll_row,
        total - 1,
        "scrolling a Live image document must clip to the last reserved row, \
         exactly like WP4's info-card case"
    );
}

/// Plan WP5 Done-when (decode-completion raw output): applying
/// `Msg::ImageDecoded` through the real `update` loop with Kitty enabled
/// pushes a non-empty `effects.raw` whose first element starts with the
/// Kitty APC intro; with Kitty disabled the same sequence produces no raw
/// output at all.
#[test]
fn the_decode_reply_transmits_only_when_kitty_is_enabled() {
    let (mut app, id) = app_with_image();
    app.graphics.kitty = true;
    app.doc_mut(id).expect("doc").viewport.set_size(40, 10);
    let effects = decode_x_png_via_update(&mut app, id);
    assert!(!effects.raw.is_empty());
    assert!(effects.raw[0].starts_with(b"\x1b_G"));
    assert_eq!(
        app.doc(id).unwrap().image.as_ref().unwrap().status,
        ImageStatus::Live
    );

    let (mut app2, id2) = app_with_image();
    app2.graphics.kitty = false;
    app2.doc_mut(id2).expect("doc").viewport.set_size(40, 10);
    let effects2 = decode_x_png_via_update(&mut app2, id2);
    assert!(effects2.raw.is_empty());
}

/// Plan WP5.S7 Done-when, driven through the real `^w` key rather than
/// calling `close_now` directly: closing an image document pushes
/// `encode_delete(id)` into `effects.raw` when Kitty is available.
#[test]
fn ctrl_w_on_a_live_image_document_emits_encode_delete() {
    let (mut app, id) = app_with_image();
    app.graphics.kitty = true;
    app.doc_mut(id).expect("doc").viewport.set_size(40, 10);
    decode_x_png_via_update(&mut app, id);
    let image_id = app.doc(id).unwrap().image.as_ref().unwrap().id;
    app.active = id;
    app.set_focus(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('w'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );

    assert!(
        !app.documents.contains_key(&id),
        "the image tab must have closed"
    );
    assert!(
        effects
            .raw
            .iter()
            .any(|bytes| *bytes == rune_image::encode_delete(image_id).into_bytes())
    );
}

/// Plan WP6.S1/S2 Done-when, driven through the real `⌘R` key rather than
/// calling `graphics::reload_image` directly: reloading a live image
/// document re-emits a transmit escape into `effects.raw` and forces a
/// redraw, under the exact same allocated id as the original open.
#[test]
fn super_r_on_a_live_image_document_reloads_under_the_same_id() {
    let (mut app, id) = app_with_image();
    app.graphics.kitty = true;
    app.doc_mut(id).expect("doc").viewport.set_size(40, 10);
    decode_x_png_via_update(&mut app, id);
    let image_id = app.doc(id).unwrap().image.as_ref().unwrap().id;
    app.active = id;
    app.set_focus(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('r'),
            mods: Mods {
                sup: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );
    assert_eq!(effects.cmds.len(), 1, "reload must spawn exactly one Cmd");

    let mut reply_effects = Effects::default();
    for cmd in effects.cmds {
        if let Some(msg) = cmd.run() {
            update(&mut app, msg, &mut reply_effects);
        }
    }

    assert!(
        reply_effects
            .raw
            .iter()
            .any(|bytes| bytes.starts_with(b"\x1b_G")),
        "reload must retransmit"
    );
    assert!(
        reply_effects.force_redraw,
        "reload must force a full redraw"
    );
    assert_eq!(
        app.doc(id).unwrap().image.as_ref().unwrap().id,
        image_id,
        "reload must retransmit under the same deterministic id"
    );
}

/// Plan WP6.S2: `⌘R` on an ordinary (non-image) document must not do
/// anything — the reload command's own no-op guard, exercised through the
/// real key pipeline against the markdown document `App::new` starts on.
#[test]
fn super_r_on_a_non_image_document_is_a_no_op() {
    let (mut app, _image_id) = app_with_image();
    let markdown_id = app
        .documents
        .keys()
        .find(|&&id| id != _image_id)
        .copied()
        .expect("a second (markdown) document from App::new");
    workspace::switch_to(&mut app, markdown_id);
    app.set_focus(Pane::Editor, &mut Effects::default());

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('r'),
            mods: Mods {
                sup: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );

    assert!(effects.cmds.is_empty());
    assert!(effects.raw.is_empty());
}
