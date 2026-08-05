#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer as CoreBuffer;
use rune_vfs::{DirEntry, Mem, Vfs};

use super::*;
use crate::runtime::{DirCause, Msg};

fn app_with(mem: &Arc<Mem>) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(CoreBuffer::new("hello"), None, vfs, None);
    app.active_doc_mut().viewport.set_size(80, 23);
    app.splits.left.show();
    app.frame_width = 80;
    app.frame_height = 24;
    app
}

fn load_entries(app: &mut App, names: &[&str]) {
    let entries: Vec<DirEntry> = names
        .iter()
        .map(|name| DirEntry {
            name: (*name).to_string(),
            path: PathBuf::from("/root").join(name),
            is_dir: false,
        })
        .collect();
    crate::explorer::handle_dir_loaded(app, PathBuf::from("/root"), entries, DirCause::Nav, 0);
}

/// Drains and runs every queued `Cmd`, feeding each `Msg` it produces
/// back into `workspace::handle_file_opened` (the real production
/// entry point for a `ReadFile` `Cmd`'s reply) — the same round trip
/// `runtime::apply` drives, minus the terminal/thread plumbing tests
/// don't need.
fn run_cmds(app: &mut App, effects: &mut Effects) {
    let cmds = std::mem::take(&mut effects.cmds);
    for cmd in cmds {
        if let Some(Msg::FileOpened {
            path,
            result,
            anchor,
        }) = cmd.run()
        {
            workspace::handle_file_opened(app, path, result, anchor, effects);
        }
    }
}

#[test]
fn arrowing_down_n_files_leaves_exactly_one_extra_tab_and_one_preview_document() {
    let mem = Arc::new(Mem::new());
    for name in ["a.md", "b.md", "c.md"] {
        mem.save_atomic(&PathBuf::from("/root").join(name), b"content")
            .unwrap();
    }
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md", "b.md", "c.md"]);
    let tabs_before = app.tabs.order.len();
    let mut effects = Effects::default();

    for _ in 0..3 {
        app.explorer.nav.move_by(1, app.explorer.entries.len());
        after_cursor_move(&mut app, &mut effects);
        run_cmds(&mut app, &mut effects);
    }

    assert_eq!(app.tabs.order.len(), tabs_before + 1);
    let preview_docs = app
        .documents
        .values()
        .filter(|d| d.read_only == ReadOnly::Preview)
        .count();
    assert_eq!(preview_docs, 1);
}

#[test]
fn enter_on_a_file_promotes_the_preview() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let id = app.explorer.preview.expect("preview minted");
    let tabs_before = app.tabs.order.len();

    promote(&mut app, id);

    assert_eq!(app.doc(id).unwrap().read_only, ReadOnly::No);
    assert!(app.explorer.preview.is_none());
    assert_eq!(app.tabs.order.len(), tabs_before);
}

#[test]
fn switching_away_discards_the_preview_from_both_collections() {
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
    let preview_id = app.explorer.preview.expect("preview minted");
    let tabs_before_preview_removed = app.tabs.order.len();

    workspace::switch_to(&mut app, real);

    assert!(app.explorer.preview.is_none());
    assert!(app.doc(preview_id).is_none(), "removed from documents");
    assert!(
        !app.tabs.order.contains(&preview_id),
        "removed from tabs.order"
    );
    assert_eq!(app.tabs.order.len(), tabs_before_preview_removed - 1);
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

    let shown_after_fresh = app
        .explorer
        .preview
        .and_then(|id| app.doc(id))
        .and_then(|d| d.file_path.clone());
    assert_eq!(shown_after_fresh, Some(PathBuf::from("/root/b.md")));

    // The late "a.md" reply must not override it.
    if let Some(Msg::FileOpened {
        path,
        result,
        anchor,
    }) = stale_cmd.run()
    {
        workspace::handle_file_opened(&mut app, path, result, anchor, &mut effects);
    }
    let shown_after_stale = app
        .explorer
        .preview
        .and_then(|id| app.doc(id))
        .and_then(|d| d.file_path.clone());
    assert_eq!(shown_after_stale, Some(PathBuf::from("/root/b.md")));
}

#[test]
fn search_live_suppresses_preview_and_clearing_it_produces_one() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    app.explorer.search = Some("a".to_string());
    let mut effects = Effects::default();

    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    assert!(effects.cmds.is_empty(), "no preview while search is live");

    app.explorer.search = None;
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
            is_dir: false,
        },
        DirEntry {
            name: "sub".to_string(),
            path: PathBuf::from("/root/sub"),
            is_dir: true,
        },
    ];
    crate::explorer::handle_dir_loaded(&mut app, PathBuf::from("/root"), entries, DirCause::Nav, 0);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "a.md"
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let previewed = app.explorer.preview;
    assert!(previewed.is_some());

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "sub"
    after_cursor_move(&mut app, &mut effects);

    assert!(effects.cmds.is_empty(), "a directory row must not read");
    assert_eq!(app.explorer.preview, previewed, "previous preview stays");
}

#[test]
fn an_oversized_file_is_skipped_with_no_read_and_no_banner() {
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

    assert!(app.explorer.preview.is_none());
    assert!(app.modal.is_none(), "no error banner for a skipped preview");
}

#[test]
fn a_binary_file_is_skipped_with_no_read_and_no_banner() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/x.bin"), &[0xFF, 0xFE, 0x00])
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["x.bin"]);
    let mut effects = Effects::default();

    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);

    assert!(app.explorer.preview.is_none());
    assert!(app.modal.is_none(), "no error banner for a skipped preview");
}

#[test]
fn ctrl_2_while_previewing_discards_and_restores_the_original_tab_count() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/c.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    // Two real tabs BEFORE the preview mints a third, so `^2` (`TabSwitch(1)`,
    // the SECOND tab) targets the pre-existing real one, not the preview.
    app.open_document(CoreBuffer::new("second"));
    let tabs_before = app.tabs.order.len();
    load_entries(&mut app, &["c.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let preview_id = app.explorer.preview.expect("preview minted");
    assert_eq!(app.tabs.order.len(), tabs_before + 1);

    let ctrl_2 = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char('2'),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    };
    crate::app::update(&mut app, Msg::Key(ctrl_2), &mut effects);

    assert_eq!(app.tabs.order.len(), tabs_before);
    assert!(app.explorer.preview.is_none());
    assert!(app.doc(preview_id).is_none(), "removed from app.documents");
}

#[test]
fn focus_entering_the_editor_promotes_the_live_preview() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let id = app.explorer.preview.expect("preview minted");
    // `on_focus_changed` only reacts to an actual TRANSITION — land on the
    // Explorer first (browsing it is what minted the preview above in the
    // first place), or `^e` below would be a no-op against the default
    // `App::new` focus, which already starts on the Editor.
    app.set_focus_pane(Pane::Explorer, &mut effects);

    let ctrl_e = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char('e'),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    };
    crate::app::update(&mut app, Msg::Key(ctrl_e), &mut effects);

    assert_eq!(app.doc(id).unwrap().read_only, ReadOnly::No);
    assert!(app.explorer.preview.is_none());
}

#[test]
fn focus_entering_the_tabs_pane_discards_the_live_preview() {
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
    let preview_id = app.explorer.preview.expect("preview minted");

    let ctrl_t = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char('t'),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    };
    crate::app::update(&mut app, Msg::Key(ctrl_t), &mut effects);

    assert!(app.explorer.preview.is_none());
    assert!(app.doc(preview_id).is_none());
    assert_eq!(app.active, real, "falls back to the surviving real tab");
    assert_eq!(app.focus(), Pane::Tabs);
}
