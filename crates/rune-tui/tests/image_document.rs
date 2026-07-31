//! WP4: opening an image file as a read-only image document — the
//! producer's synthesized rows actually scroll, the save/highlight guards
//! actually fire, and the info card renders on a non-Kitty terminal.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_syntax::DocumentKind;
use rune_tui::app::{App, update};
use rune_tui::commands::nav_scroll;
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
/// `trigger_save` guard a stale `saved_version` (simulating a would-be
/// dirty state) would reach the no-DB-binding fallback and push a `Save`
/// `Cmd` that overwrites the image with the buffer's own (always empty)
/// bytes. Driven through the real `super+s` key via `app::update`, per the
/// plan's "drive integration tests through the real update loop".
#[test]
fn super_s_on_an_image_document_saves_nothing_and_never_focuses_the_title() {
    let (mut app, id) = app_with_image();
    // Force the version check an earlier revision of the guard would have
    // relied on to look "dirty" — the guard must still fire BEFORE it.
    app.doc_mut(id).expect("doc").saved_version =
        app.doc(id).expect("doc").buffer.version().wrapping_sub(1);
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
