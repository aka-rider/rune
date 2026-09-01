#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use rune_vfs::VfsTestExt;
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer as CoreBuffer;
use rune_vfs::Mem;

use super::tests_common::{app_with, load_entries, run_cmds};

fn resolved(app: &App, path: &str) -> crate::resolved::ResolvedPath {
    crate::resolved::ResolvedPath::resolve(app.vfs.as_ref(), std::path::Path::new(path))
        .expect("Mem resolves any spelling")
}
use super::*;
use crate::document::ReadOnly;
use crate::runtime::Msg;

#[test]
fn a_preview_is_not_a_workspace_document() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);

    let id = app.explorer.preview.as_ref().expect("preview minted").id;
    assert!(app.doc(id).is_none(), "not in app.documents");
    assert!(!app.documents.order().contains(&id), "not in the tab list");
    assert!(!app.documents.mru().contains(&id), "not in the MRU");
    assert!(
        !app.nav_history.places().iter().any(|place| place.doc == id),
        "not a nav-history place"
    );
    assert!(
        crate::pane_quit::unpreserved_dirty_docs(&mut app).is_empty(),
        "a preview never blocks quit"
    );
}

#[test]
fn a_preview_costs_no_tab_even_with_the_tab_strip_full() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    while app.documents.order().len() < crate::opentabs::limit::MAX_TABS {
        let n = app.documents.order().len() + 1;
        let path = resolved(&app, &format!("/root/filler{n}.md"));
        app.open_document_bound(CoreBuffer::new("filler"), path);
    }
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    let docs_before = app.documents.len();

    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);

    assert_eq!(shown_path(&app), Some(std::path::Path::new("/root/a.md")));
    assert_eq!(
        app.documents.len(),
        docs_before,
        "a full tab strip still previews, and still mints no document"
    );
}

#[test]
fn promoting_the_preview_opens_it_as_a_tab() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let tabs_before = app.documents.order().len();

    let promoted = match promote(&mut app, &mut effects) {
        Promotion::Promoted(id) => Some(id),
        Promotion::NothingToPromote | Promotion::Refused => None,
    };
    let id = promoted.expect("the preview is promoted");

    assert_eq!(app.doc(id).unwrap().read_only, ReadOnly::No);
    assert!(app.explorer.preview.is_none());
    assert_eq!(app.documents.order().len(), tabs_before + 1);
    assert_eq!(app.active, id);
}

#[test]
fn switching_to_a_real_document_discards_the_preview() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    let real = app.active;
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let tabs_before = app.documents.order().len();

    workspace::switch_to(&mut app, real);

    assert!(app.explorer.preview.is_none());
    assert_eq!(app.documents.order().len(), tabs_before);
}

#[test]
fn an_explorer_round_trip_leaves_an_edited_documents_journal_intact() {
    let mem = Arc::new(Mem::new());
    for name in ["a.md", "b.md"] {
        mem.save_atomic(&PathBuf::from("/root").join(name), b"content")
            .unwrap();
    }
    let mut app = app_with(&mem);
    let mut effects = Effects::default();
    let edited = workspace::open_path(&mut app, std::path::Path::new("/root/a.md")).unwrap();
    crate::app::update(
        &mut app,
        Msg::Key(crate::keymap::KeyInput {
            code: crate::keymap::KeyCode::Char('x'),
            mods: crate::keymap::Mods::NONE,
        }),
        &mut effects,
    );
    let journal_before = app.doc(edited).unwrap().journal.pos();
    let content_before = app.doc(edited).unwrap().buffer.content().to_string();
    assert!(journal_before > 0, "test setup: a.md has undo history");

    load_entries(&mut app, &["a.md", "b.md"]);
    app.set_focus_pane(crate::pane::Pane::Explorer, &mut effects);
    for _ in 0..2 {
        app.explorer.nav.move_by(1, app.explorer.entries.len());
        after_cursor_move(&mut app, &mut effects);
        run_cmds(&mut app, &mut effects);
    }
    assert_eq!(shown_path(&app), Some(std::path::Path::new("/root/b.md")));

    app.explorer.nav.move_by(-1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);

    assert_eq!(app.active, edited, "arrowing back reactivates the real tab");
    assert_eq!(app.doc(edited).unwrap().journal.pos(), journal_before);
    assert_eq!(app.doc(edited).unwrap().buffer.content(), content_before);
}

#[test]
fn undo_while_a_preview_is_showing_reaches_the_real_document_only() {
    let mem = Arc::new(Mem::new());
    for name in ["a.md", "b.md"] {
        mem.save_atomic(&PathBuf::from("/root").join(name), b"content")
            .unwrap();
    }
    let mut app = app_with(&mem);
    let mut effects = Effects::default();
    let edited = workspace::open_path(&mut app, std::path::Path::new("/root/a.md")).unwrap();
    crate::app::update(
        &mut app,
        Msg::Key(crate::keymap::KeyInput {
            code: crate::keymap::KeyCode::Char('x'),
            mods: crate::keymap::Mods::NONE,
        }),
        &mut effects,
    );
    assert_eq!(app.doc(edited).unwrap().buffer.content(), "xcontent");

    load_entries(&mut app, &["a.md", "b.md"]);
    app.set_focus_pane(crate::pane::Pane::Explorer, &mut effects);
    for _ in 0..2 {
        app.explorer.nav.move_by(1, app.explorer.entries.len());
        after_cursor_move(&mut app, &mut effects);
        run_cmds(&mut app, &mut effects);
    }
    let preview_content = app
        .explorer
        .preview
        .as_ref()
        .expect("b.md previewed")
        .doc
        .buffer
        .content()
        .to_string();

    let active = app.active;
    crate::commands::edit::undo(&mut app, active);

    assert_eq!(
        app.doc(edited).unwrap().buffer.content(),
        "content",
        "^z undoes the document the user is editing"
    );
    let preview = app.explorer.preview.as_ref().expect("preview still live");
    assert_eq!(preview.doc.buffer.content(), preview_content);
    assert_eq!(
        preview.doc.journal.pos(),
        0,
        "a preview has no history to undo into"
    );
}

#[test]
fn promoting_at_a_full_pinned_tab_strip_refuses_with_the_tab_limit_warning() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    app.active_doc_mut().pinned = true;
    while app.documents.order().len() < crate::opentabs::limit::MAX_TABS {
        let n = app.documents.order().len() + 1;
        let path = resolved(&app, &format!("/root/filler{n}.md"));
        let id = app.open_document_bound(CoreBuffer::new("filler"), path);
        app.doc_mut(id).unwrap().pinned = true;
    }
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let tabs_before = app.documents.order().len();

    assert_eq!(
        promote(&mut app, &mut effects),
        Promotion::Refused,
        "at an unevictable cap a promotion is refused"
    );

    assert_eq!(app.documents.order().len(), tabs_before, "no tab opened");
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("Tab limit reached \u{2014} close or unpin a tab"),
        "a refused promotion must say why"
    );
}
