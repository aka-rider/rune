#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer as CoreBuffer;
use rune_vfs::{Mem, Vfs, VfsTestExt};

use super::tests_common::{load_entries, run_cmds};
use super::*;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::pointer::{MouseButton, MouseInput, MouseKind};
use crate::runtime::Msg;

const OPEN_BODY: &str = "the open document\nsecond line of it\n";
const PREVIEW_BODY: &str = "the previewed file\nand its second line\n";

fn long_body(tag: &str) -> String {
    (0..200).map(|n| format!("{tag} line {n}\n")).collect()
}

fn app_showing_preview(preview_body: &str, open_body: &str) -> (App, DocumentId) {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/preview.md"), preview_body.as_bytes())
        .unwrap();
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        CoreBuffer::new(open_body),
        Some(
            crate::resolved::ResolvedPath::resolve(
                vfs.as_ref(),
                std::path::Path::new(&PathBuf::from("/root/open.md")),
            )
            .expect("the launch path resolves"),
        ),
        vfs,
        None,
    );
    app.splits.left.show();
    app.frame = Some(crate::app::FrameSize::new(80, 24));
    let open = app.active;
    let mut effects = Effects::default();
    app.set_focus_pane(Pane::Explorer, &mut effects);
    load_entries(&mut app, &["preview.md"]);
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    assert_eq!(
        shown_path(&app),
        Some(Path::new("/root/preview.md")),
        "test setup: the Explorer is previewing preview.md"
    );
    app.sync_view();
    (app, open)
}

fn mouse(app: &mut App, kind: MouseKind, dx: u16, dy: u16) {
    let editor = crate::layout::geometry(app.frame_area(), app).editor;
    let mut effects = Effects::default();
    crate::app::update(
        app,
        Msg::Mouse(MouseInput {
            kind,
            column: editor.x + dx,
            row: editor.y + dy,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
}

struct Chrome {
    title: String,
    breadcrumb: String,
    footer: String,
}

fn chrome(app: &App) -> Chrome {
    let geo = crate::layout::geometry(app.frame_area(), app);
    let grid = crate::testgrid::grid(app, 80, 24);
    let title = geo.title.map(|rect| rect.y).expect("a title row");
    let breadcrumb = geo.center.y + geo.center.height - 1;
    Chrome {
        title: grid[title as usize].clone(),
        breadcrumb: grid[breadcrumb as usize].clone(),
        footer: grid[geo.footer.y as usize].clone(),
    }
}

#[test]
fn arrowing_onto_a_file_paints_it_while_the_active_document_stays_put() {
    let (app, open) = app_showing_preview(PREVIEW_BODY, OPEN_BODY);

    let grid = crate::testgrid::grid(&app, 80, 24);
    assert!(
        grid.iter().any(|row| row.contains("the previewed file")),
        "the previewed file's own text must reach the screen: {grid:#?}"
    );
    assert!(
        !grid.iter().any(|row| row.contains("the open document")),
        "the hidden active document must not be painted: {grid:#?}"
    );
    assert_eq!(
        app.active, open,
        "browsing never renames the active document"
    );
    assert_eq!(app.active_doc().buffer.content(), OPEN_BODY);
}

#[test]
fn scrolling_over_a_painted_preview_scrolls_the_preview_and_not_the_hidden_document() {
    let (mut app, open) = app_showing_preview(&long_body("preview"), &long_body("open"));
    let hidden_scroll_before = app.doc(open).unwrap().viewport.scroll_row;
    let hidden_caret_before = app.doc(open).unwrap().cursors.primary().position;
    let preview_scroll_before = app.shown_doc().viewport.scroll_row;

    mouse(&mut app, MouseKind::ScrollDown, 1, 1);
    app.sync_view();

    assert!(
        app.shown_doc().viewport.scroll_row > preview_scroll_before,
        "the wheel must move the document the user can see"
    );
    assert_eq!(
        app.doc(open).unwrap().viewport.scroll_row,
        hidden_scroll_before,
        "the hidden document's viewport must not move"
    );
    assert_eq!(
        app.doc(open).unwrap().cursors.primary().position,
        hidden_caret_before
    );
}

#[test]
fn clicking_a_painted_preview_opens_it_instead_of_moving_the_hidden_caret() {
    let (mut app, open) = app_showing_preview(PREVIEW_BODY, OPEN_BODY);
    let previewed = app.explorer.preview.as_ref().unwrap().id;
    let hidden_caret_before = app.doc(open).unwrap().cursors.primary().position;
    let hidden_scroll_before = app.doc(open).unwrap().viewport.scroll_row;

    mouse(&mut app, MouseKind::Down(MouseButton::Left), 4, 0);

    assert_eq!(
        app.active, previewed,
        "clicking what is on screen opens it rather than acting on a hidden document"
    );
    assert_eq!(
        app.doc(open).unwrap().cursors.primary().position,
        hidden_caret_before,
        "the previously active document's caret must be untouched"
    );
    assert_eq!(
        app.doc(open).unwrap().viewport.scroll_row,
        hidden_scroll_before
    );
    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        4,
        "the click lands on the document it was aimed at"
    );
}

#[test]
fn the_chrome_names_the_preview_and_withholds_the_caret_readout() {
    let (mut app, _open) = app_showing_preview(PREVIEW_BODY, OPEN_BODY);
    let mut effects = Effects::default();
    app.set_focus_pane(Pane::Editor, &mut effects);
    crate::app::update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('x'),
            mods: Mods::NONE,
        }),
        &mut effects,
    );
    app.set_focus_pane(Pane::Explorer, &mut effects);
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    app.sync_view();
    assert!(
        app.active_doc().is_dirty(),
        "test setup: the hidden document has unsaved work"
    );
    assert!(
        app.explorer.preview.is_some(),
        "test setup: a preview is on screen"
    );

    let previewing = chrome(&app);
    assert!(
        previewing.title.contains("preview.md"),
        "the title names the file on screen: {:?}",
        previewing.title
    );
    assert!(
        !previewing.title.contains('\u{2022}'),
        "a preview cannot be dirty, so it carries no dirty marker: {:?}",
        previewing.title
    );
    assert!(
        previewing.breadcrumb.contains("preview.md"),
        "the breadcrumb names the previewed file: {:?}",
        previewing.breadcrumb
    );
    assert!(
        crate::footer::position_text(&app).is_none(),
        "a preview has no caret, so there is no Ln/Col to report"
    );
    assert!(
        !previewing.footer.contains("Ln "),
        "the footer withholds the readout while previewing: {:?}",
        previewing.footer
    );

    discard(&mut app);
    app.sync_view();

    let restored = chrome(&app);
    assert!(
        restored.title.contains("open.md"),
        "discarding returns the title to the active document: {:?}",
        restored.title
    );
    assert!(
        restored.title.contains('\u{2022}'),
        "and returns its dirty marker: {:?}",
        restored.title
    );
    assert!(
        restored.breadcrumb.contains("open.md"),
        "and returns the breadcrumb: {:?}",
        restored.breadcrumb
    );
    assert!(
        crate::footer::position_text(&app).is_some(),
        "and returns the caret readout"
    );
    assert!(restored.footer.contains("Ln "), "{:?}", restored.footer);
}
