#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use rune_vfs::VfsTestExt;
use std::path::PathBuf;
use std::sync::Arc;

use rune_vfs::{DirEntry, FileKind, Mem, Vfs};

use super::tests_common::{app_with, load_entries, run_cmds};
use super::*;
use crate::document::ReadOnly;
use crate::runtime::{DirCause, Msg};

#[test]
fn arrowing_down_n_files_mints_no_tab_and_leaves_exactly_one_preview() {
    let mem = Arc::new(Mem::new());
    for name in ["a.md", "b.md", "c.md"] {
        mem.save_atomic(&PathBuf::from("/root").join(name), b"content")
            .unwrap();
    }
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md", "b.md", "c.md"]);
    let tabs_before = app.documents.order().len();
    let active_before = app.active;
    let mut effects = Effects::default();

    for _ in 0..3 {
        app.explorer.nav.move_by(1, app.explorer.entries.len());
        after_cursor_move(&mut app, &mut effects);
        run_cmds(&mut app, &mut effects);
    }

    assert_eq!(
        app.documents.order().len(),
        tabs_before,
        "browsing three files must open no tab"
    );
    assert_eq!(app.active, active_before, "browsing never moves the caret");
    assert_eq!(shown_path(&app), Some(std::path::Path::new("/root/c.md")));
}

#[test]
fn cursoring_onto_an_already_open_document_shows_it_without_closing_it_on_move_away() {
    let mem = Arc::new(Mem::new());
    for name in ["a.md", "b.md"] {
        mem.save_atomic(&PathBuf::from("/root").join(name), b"content")
            .unwrap();
    }
    let mut app = app_with(&mem);
    let opened = workspace::open_path(&mut app, std::path::Path::new("/root/a.md")).unwrap();
    app.doc_mut(opened).unwrap().buffer = rune_core::buffer::Buffer::new("content\nedited");
    load_entries(&mut app, &["a.md", "b.md"]);
    let mut effects = Effects::default();

    // Cursor starts on the synthetic ".." row; one Down lands on "a.md".
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);

    assert_eq!(app.active, opened);
    assert!(app.explorer.preview.is_none(), "nothing minted to reuse");

    // Arrowing away must not close the real document.
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);

    assert!(app.doc(opened).is_some());
}

#[test]
fn a_stale_reply_for_a_path_the_cursor_has_left_is_ignored() {
    let mem = Arc::new(Mem::new());
    for name in ["a.md", "b.md"] {
        mem.save_atomic(&PathBuf::from("/root").join(name), b"content")
            .unwrap();
    }
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md", "b.md"]);
    let mut effects = Effects::default();

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "a.md"
    after_cursor_move(&mut app, &mut effects);
    let stale_cmd = effects.cmds.pop().expect("a.md read queued");

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "b.md"
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects); // resolves "b.md" first

    assert_eq!(shown_path(&app), Some(std::path::Path::new("/root/b.md")));

    // The late "a.md" reply must not override it.
    if let Some(Msg::FileOpened {
        path,
        result,
        anchor,
        preview_generation,
    }) = stale_cmd.run()
    {
        workspace::handle_file_opened(
            &mut app,
            &path,
            result,
            anchor,
            preview_generation,
            &mut effects,
        );
    }
    assert_eq!(shown_path(&app), Some(std::path::Path::new("/root/b.md")));
}

#[test]
fn search_live_suppresses_preview_and_clearing_it_produces_one() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    app.explorer_find_push('a');
    let mut effects = Effects::default();

    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    assert!(effects.cmds.is_empty(), "no preview while search is live");

    app.close_explorer_find();
    after_cursor_move(&mut app, &mut effects);
    assert_eq!(effects.cmds.len(), 1, "clearing the search previews once");
}

#[test]
fn a_directory_row_produces_no_read_and_keeps_the_previous_preview() {
    let mem = Arc::new(Mem::new());
    mem.mkdir_all(std::path::Path::new("/root/sub")).unwrap();
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    let entries = vec![
        DirEntry {
            name: "a.md".to_string(),
            path: PathBuf::from("/root/a.md"),
            kind: FileKind::File,
            link: rune_vfs::Link::No,
        },
        DirEntry {
            name: "sub".to_string(),
            path: PathBuf::from("/root/sub"),
            kind: FileKind::Dir,
            link: rune_vfs::Link::No,
        },
    ];
    crate::explorer::handle_dir_loaded(
        &mut app,
        PathBuf::from("/root"),
        entries,
        DirCause::Nav,
        crate::generation::Generation::ZERO,
    );
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "a.md"
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    assert_eq!(shown_path(&app), Some(std::path::Path::new("/root/a.md")));

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "sub"
    after_cursor_move(&mut app, &mut effects);

    assert!(effects.cmds.is_empty(), "a directory row must not read");
    assert_eq!(
        shown_path(&app),
        Some(std::path::Path::new("/root/a.md")),
        "previous preview stays"
    );
}

#[test]
fn an_oversized_file_renders_a_placeholder_with_no_banner() {
    let mem = Arc::new(Mem::new());
    let huge = vec![b'x'; crate::runtime::MAX_PREVIEW_BYTES as usize + 1];
    mem.save_atomic(std::path::Path::new("/root/huge.md"), &huge)
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["huge.md"]);
    let mut effects = Effects::default();

    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);

    let preview = app.explorer.preview.as_ref().expect("a placeholder");
    assert!(preview.doc.buffer.content().contains("huge.md"));
    assert!(
        preview
            .doc
            .buffer
            .content()
            .contains("too large to preview")
    );
    assert!(preview.doc.path().is_none(), "a placeholder is never bound");
    assert!(
        !crate::messages::is_open(&app),
        "no message pane for a failed preview"
    );
}

#[test]
fn a_binary_file_renders_a_placeholder_with_no_banner() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/x.bin"), &[0xFF, 0xFE, 0x00])
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["x.bin"]);
    let mut effects = Effects::default();

    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);

    let preview = app.explorer.preview.as_ref().expect("a placeholder");
    assert!(preview.doc.buffer.content().contains("x.bin"));
    assert!(preview.doc.buffer.content().contains("not valid UTF-8"));
    assert!(preview.doc.path().is_none(), "a placeholder is never bound");
    assert!(
        !crate::messages::is_open(&app),
        "no message pane for a failed preview"
    );
}

/// Defect 1's own race: the explorer's live preview and a real, anchored
/// file open (e.g. following a link) both target the same path, and the
/// REAL open's own reply lands first, while the preview's is still in
/// flight. Correlated purely by path (the old bug), the real open's own
/// bytes would be swallowed as the preview's — landing read-only, its
/// anchor dropped. Correlated by generation, the real open must land as an
/// ordinary document with its anchor applied, and the stale preview reply
/// that lands after must change nothing about it.
#[test]
fn a_real_open_racing_its_own_in_flight_preview_lands_with_its_anchor() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"first\nsecond\nthird")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "a.md"
    after_cursor_move(&mut app, &mut effects);
    let preview_cmd = effects.cmds.pop().expect("preview cmd queued");

    workspace::open_path_async(
        &mut app,
        std::path::Path::new("/root/a.md"),
        Some(rune_nav::Anchor::Line(2)),
        &mut effects,
    );
    let real_open_cmd = effects.cmds.pop().expect("real open cmd queued");

    // The race: the real open's own reply lands first.
    if let Some(Msg::FileOpened {
        path,
        result,
        anchor,
        preview_generation,
    }) = real_open_cmd.run()
    {
        workspace::handle_file_opened(
            &mut app,
            &path,
            result,
            anchor,
            preview_generation,
            &mut effects,
        );
    }

    let id = app.active;
    assert_eq!(
        app.doc(id).map(|d| d.read_only),
        Some(ReadOnly::No),
        "a real open racing its own preview must land as a normal document, \
         never be swallowed as the stale preview's own reply"
    );
    assert_eq!(app.focus(), Pane::Editor, "the real open must move focus");
    let landed_at = app.doc(id).map(|d| d.cursors.primary().position.get());
    assert_eq!(
        landed_at,
        Some("first\n".len()),
        "the anchor from the real open must land, not be dropped"
    );

    // The stale preview reply lands after — it must be dropped, never
    // downgrading the document the real open just landed.
    if let Some(Msg::FileOpened {
        path,
        result,
        anchor,
        preview_generation,
    }) = preview_cmd.run()
    {
        workspace::handle_file_opened(
            &mut app,
            &path,
            result,
            anchor,
            preview_generation,
            &mut effects,
        );
    }
    assert_eq!(app.doc(id).map(|d| d.read_only), Some(ReadOnly::No));
    assert_eq!(
        app.doc(id).map(|d| d.cursors.primary().position.get()),
        landed_at
    );
}

/// The finding this guards: `discard()` used to drop the preview without
/// invalidating the in-flight read behind it. A user arrowing onto a file
/// mints generation G and spawns its read, then switches to a real tab
/// before the read lands — `switch_to` calls `discard`. If the reply for G
/// still lands, it must die on arrival rather than re-installing a preview
/// over the document the user switched to.
#[test]
fn switching_tabs_before_a_preview_read_lands_kills_the_stale_reply() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"a content")
        .unwrap();
    mem.save_atomic(std::path::Path::new("/root/b.md"), b"b content")
        .unwrap();
    let mut app = app_with(&mem);
    let opened = workspace::open_path(&mut app, std::path::Path::new("/root/a.md")).unwrap();
    load_entries(&mut app, &["a.md", "b.md"]);
    let mut effects = Effects::default();

    app.explorer.nav.move_by(2, app.explorer.entries.len()); // "b.md"
    after_cursor_move(&mut app, &mut effects);
    let preview_cmd = effects.cmds.pop().expect("b.md read queued");
    assert!(app.explorer.preview_awaiting.is_some());

    // The user clicks the "a.md" tab before the read comes back.
    workspace::switch_to(&mut app, opened);
    assert!(
        app.explorer.preview.is_none(),
        "switching tabs must drop the preview"
    );
    assert!(
        app.explorer.preview_awaiting.is_none(),
        "switching tabs must stop waiting on the now-discarded read"
    );

    if let Some(Msg::FileOpened {
        path,
        result,
        anchor,
        preview_generation,
    }) = preview_cmd.run()
    {
        workspace::handle_file_opened(
            &mut app,
            &path,
            result,
            anchor,
            preview_generation,
            &mut effects,
        );
    }

    assert!(
        app.explorer.preview.is_none(),
        "the stale reply must not re-install a preview over the switched-to tab"
    );
    assert_eq!(app.shown(), opened, "the active document stays on screen");
    assert_eq!(app.active, opened);
}
