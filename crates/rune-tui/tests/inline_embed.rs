//! Inline image embeds inside an otherwise-editable markdown
//! document — placement, caret protection, spawn/despawn, and the
//! mtime-respawn retry rule. Driven through the real `app::update` loop
//! ("drive integration tests through the real
//! update loop, with a fresh `Effects::default()` per message").
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::{CursorSet, CursorSpec};
use rune_tui::app::update;
use rune_tui::graphics::ImageStatus;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{CmdError, CmdKind, Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::VfsTestExt;

mod inline_embed_common;
use inline_embed_common::{X_PNG, app_with_embed, discover_and_decode};

/// A standalone image line renders placeholder cells
/// whose `fg` encodes the allocated id, once Kitty is enabled and the
/// decode has landed.
#[test]
fn a_standalone_image_line_renders_placeholder_cells_with_the_allocated_id() {
    let (mut app, id) = app_with_embed("![caption](x.png)\n");
    discover_and_decode(&mut app);

    let doc = app.doc(id).expect("doc");
    let embed = doc
        .embeds()
        .expect("embeds tracked")
        .images
        .get("x.png")
        .expect("embed tracked");
    assert!(
        matches!(embed.status, ImageStatus::Live { .. }),
        "decode must have landed"
    );
    let expected_id = embed.id.get();

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

/// The same line with Kitty disabled renders alt text
/// instead — `sync_embeds` never spawns anything (gated on
/// `app.graphics.kitty`), and `render::image::row_cells` returns `None`
/// for the row, falling through to its ordinary alt-text span.
#[test]
fn the_same_line_with_kitty_disabled_renders_alt_text() {
    let (mut app, _id) = app_with_embed("![caption](x.png)\n");
    app.graphics.kitty = false;
    let mut effects = Effects::default();
    update(&mut app, Msg::Resize(60, 20), &mut effects);
    assert!(
        effects
            .cmds
            .iter()
            .all(|c| c.kind() != CmdKind::ImageDecode),
        "no Kitty means sync_embeds must never spawn a decode"
    );
    app.sync_view();

    let frame = testgrid::grid(&app, 60, 20).join("\n");
    assert!(
        frame.contains("caption"),
        "frame did not fall back to alt text:\n{frame}"
    );
}

/// A caret sitting at the END of the image's own
/// markdown syntax — past `range.end`, so the image stays `Rendered`
/// rather than revealing to raw source, but still visually "on" the
/// placeholder row via `visual_col` — must leave every placeholder cell's
/// `fg` intact and add no `REVERSED` modifier. This is the exact
/// regression `place_caret`'s blind positional cell walk would otherwise
/// cause: `visual_col` is derived from the row's ALT-TEXT length (its real
/// wrap-space content), which can differ from the placeholder row's own
/// cell count, so `place_caret` would land on and reverse a real
/// placeholder cell — corrupting its smuggled id — without this
/// suppression.
#[test]
fn a_caret_on_the_image_row_leaves_every_placeholder_cells_fg_intact() {
    let (mut app, id) = app_with_embed("![caption](x.png) \n");
    discover_and_decode(&mut app);

    // Trailing whitespace-only text after the image still counts as
    // standalone (does not disqualify the image row).
    let end_of_line = app
        .doc(id)
        .expect("doc")
        .buffer
        .content()
        .find('\n')
        .expect("newline");
    {
        let doc = app.doc_mut(id).expect("doc");
        doc.cursors = CursorSet::new_from_specs(&[CursorSpec {
            position: BufferOffset(end_of_line),
            anchor: BufferOffset(end_of_line),
            desired_col: VisualCol(0),
        }]);
    }
    app.sync_view();

    let view = app.doc(id).expect("doc").view.clone().expect("view");
    assert!(
        view.display.rows().iter().any(|r| r.image.is_some()),
        "test setup: the image must still be an image row (Rendered, not revealed) \
         with the caret at this offset — got no image row at all:\n{:?}",
        view.display
            .rows()
            .iter()
            .map(|r| r.synthetic)
            .collect::<Vec<_>>()
    );

    let embed = app
        .doc(id)
        .expect("doc")
        .embeds()
        .expect("embeds tracked")
        .images
        .get("x.png")
        .expect("embed tracked");
    let expected_id = embed.id.get();
    let (r, g, b) = (
        ((expected_id >> 16) & 0xFF) as u8,
        ((expected_id >> 8) & 0xFF) as u8,
        (expected_id & 0xFF) as u8,
    );
    let expected_fg = ratatui::style::Color::Rgb(r, g, b);

    let buf = testgrid::draw(&app, 60, 20);
    let placeholder = rune_image::PLACEHOLDER;
    let mut saw_placeholder = false;
    for y in 0..20 {
        for x in 0..60 {
            let Some(cell) = buf.cell((x, y)) else {
                continue;
            };
            if cell.symbol().starts_with(placeholder) {
                saw_placeholder = true;
                assert_eq!(
                    cell.fg, expected_fg,
                    "a placeholder cell's fg was corrupted by the caret overlay"
                );
                assert!(
                    !cell.modifier.contains(ratatui::style::Modifier::REVERSED),
                    "the caret must never paint REVERSED onto an image row"
                );
            }
        }
    }
    assert!(saw_placeholder, "test setup: no placeholder cell rendered");
}

/// Moving the caret onto the embed line (revealing
/// its raw source) must never despawn a live image; deleting the line
/// entirely must.
#[test]
fn moving_the_caret_onto_the_line_keeps_the_image_live_deleting_the_line_despawns_it() {
    let (mut app, id) = app_with_embed("text before\n\n![caption](x.png)\n");
    discover_and_decode(&mut app);
    assert!(
        app.doc(id)
            .expect("doc")
            .embeds()
            .expect("embeds tracked")
            .images
            .contains_key("x.png"),
        "test setup: the embed must have spawned"
    );

    // Move the caret onto the image's own line — well inside its byte
    // range, so this line reveals to raw markdown source.
    let content = app.doc(id).expect("doc").buffer.content().to_string();
    let image_line_start = content.find("![caption]").expect("image line");
    {
        let doc = app.doc_mut(id).expect("doc");
        doc.cursors = CursorSet::new_from_specs(&[CursorSpec {
            position: BufferOffset(image_line_start + 2),
            anchor: BufferOffset(image_line_start + 2),
            desired_col: VisualCol(0),
        }]);
    }
    let mut effects = Effects::default();
    update(&mut app, Msg::Resize(61, 20), &mut effects);
    assert!(
        app.doc(id)
            .expect("doc")
            .embeds()
            .expect("embeds tracked")
            .images
            .contains_key("x.png"),
        "revealing the embed's own line must never despawn it"
    );

    // Now delete the whole image line's content — the embed target is no
    // longer present anywhere in the document at all.
    {
        let doc = app.doc_mut(id).expect("doc");
        let new_content = "text before\n\n";
        let edits = rune_core::buffer::SortedEdits::single(rune_core::buffer::Edit {
            start: 0,
            end: doc.buffer.len(),
            insert: new_content.to_string(),
        });
        let (new_buffer, _applied) = doc.buffer.apply_edits(&edits).expect("edit applies");
        doc.buffer = new_buffer;
        doc.cursors = CursorSet::new(0);
    }
    // Reparse BEFORE the next `update` call: `sync_embeds` reads
    // `doc.doc.blocks()`, which only a `view()`/`sync_view()` call
    // refreshes — a raw buffer mutation outside the normal edit
    // chokepoint (as this test makes directly) never does that on its
    // own, exactly like `app_with_embed`'s own doc comment already notes.
    app.sync_view();
    let mut effects2 = Effects::default();
    update(&mut app, Msg::Resize(62, 20), &mut effects2);
    assert!(
        !app.doc(id)
            .expect("doc")
            .embeds()
            .is_some_and(|embeds| embeds.images.contains_key("x.png")),
        "deleting the embed's line must despawn it"
    );
    assert!(
        effects2
            .raw_bytes()
            .iter()
            .any(|bytes| bytes.starts_with(b"\x1b_Gq=2,")),
        "despawning must push an encode_delete escape into effects.raw_bytes()"
    );
}

/// `Failed` is sticky per `(path, mtime)` — an
/// unchanged mtime never respawns a failed embed; a genuine mtime change
/// always does.
#[test]
fn an_unchanged_mtime_after_a_failure_does_not_respawn_a_changed_mtime_does() {
    let (mut app, id) = app_with_embed("![caption](x.png)\n");

    // First discovery: fail the decode deliberately.
    let mut effects = Effects::default();
    update(&mut app, Msg::Resize(60, 20), &mut effects);
    let mut failed_any = false;
    for cmd in effects.cmds {
        if cmd.kind() != CmdKind::ImageDecode {
            continue;
        }
        if let Some(Msg::EmbedDecoded {
            doc, generation, ..
        }) = cmd.run()
        {
            let mut reply_effects = Effects::default();
            update(
                &mut app,
                Msg::EmbedDecoded {
                    doc,
                    generation,
                    result: Err(CmdError::Refused("boom".to_string())),
                },
                &mut reply_effects,
            );
            failed_any = true;
        }
    }
    assert!(
        failed_any,
        "test setup: the decode must have been forced to fail"
    );
    assert!(matches!(
        app.doc(id)
            .expect("doc")
            .embeds()
            .expect("embeds tracked")
            .images
            .get("x.png")
            .expect("tracked")
            .status,
        ImageStatus::Failed(_)
    ));

    // Reconcile again with the SAME mtime (the fixture file untouched):
    // must not respawn — the decode `Cmd` was never re-armed, so no new
    // in-flight generation should appear.
    let mut effects2 = Effects::default();
    update(&mut app, Msg::Resize(61, 20), &mut effects2);
    assert!(
        effects2
            .cmds
            .iter()
            .all(|c| c.kind() != CmdKind::ImageDecode),
        "an unchanged mtime after a failure must never respawn"
    );
    assert!(matches!(
        app.doc(id)
            .expect("doc")
            .embeds()
            .expect("embeds tracked")
            .images
            .get("x.png")
            .expect("tracked")
            .status,
        ImageStatus::Failed(_)
    ));

    // Now genuinely change the mtime (Mem's own synthetic tick counter
    // advances on every write — deterministic, no wall-clock sleep needed)
    // and reconcile again: this MUST respawn.
    app.vfs
        .save_atomic(Path::new("/vault/x.png"), X_PNG)
        .expect("rewrite x.png to bump its mtime");
    let mut effects3 = Effects::default();
    update(&mut app, Msg::Resize(62, 20), &mut effects3);
    assert!(
        effects3
            .cmds
            .iter()
            .any(|c| c.kind() == CmdKind::ImageDecode),
        "a genuine mtime change must respawn regardless of the prior Failed state"
    );
}

/// The geometry-only mouse hit-testing path and
/// `build_rows` agree on cell count for an image row — both go through the
/// SAME `render::image::row_cells` chokepoint ("mirror the
/// override into the geometry-only variant, or mouse coordinates will
/// disagree with what is drawn").
#[test]
fn the_geometry_variant_and_build_rows_agree_on_cell_count_for_an_image_row() {
    let (mut app, id) = app_with_embed("![caption](x.png)\n");
    discover_and_decode(&mut app);

    let doc = app.doc(id).expect("doc");
    let view = doc.view.clone().expect("view");
    let image_ref = view
        .display
        .rows()
        .iter()
        .find_map(|r| r.image.clone())
        .expect("an image row must exist");

    let width = doc.viewport.width;
    let from_build_rows = rune_tui::render::image::row_cells(&app, doc, &image_ref, width)
        .expect("a live, Kitty-capable row is Some")
        .len();
    let from_geometry = rune_tui::render::image::row_cells(&app, doc, &image_ref, width)
        .expect("the same call must agree with itself")
        .len();
    assert_eq!(
        from_build_rows, from_geometry,
        "build_rows and the geometry-only mouse path must draw the same cell count"
    );
}

/// Review finding (formerly-silent `Command::Reload`): once a markdown
/// document's only embed has finished decoding (`ImageStatus::Live`, no
/// `in_flight` left to reschedule), `⌘R` must refuse with the same status
/// message an embed-less document gets — `Document::has_reloadable_graphics`
/// is the single predicate `dispatch::Command::Reload`'s gate and `reload_
/// embeds`'s own rescheduling both read, so neither can drift from the
/// other into a reload that silently does nothing.
#[test]
fn super_r_on_a_document_with_only_live_embeds_refuses_with_a_message() {
    let (mut app, id) = app_with_embed("![caption](x.png)\n");
    discover_and_decode(&mut app);
    assert!(
        matches!(
            app.doc(id)
                .expect("doc")
                .embeds()
                .expect("embeds tracked")
                .images
                .get("x.png")
                .expect("embed tracked")
                .status,
            ImageStatus::Live { .. }
        ),
        "the embed must have finished decoding before this test's own assertion means anything"
    );

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

    assert!(
        !effects
            .cmds
            .iter()
            .any(|c| c.kind() == CmdKind::ImageDecode),
        "no decode Cmd may be armed for an all-Live embed set"
    );
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        Some("nothing to reload")
    );
}
