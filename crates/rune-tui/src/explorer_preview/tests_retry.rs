#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use rune_vfs::VfsTestExt;
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer as CoreBuffer;
use rune_vfs::Mem;

use super::tests_common::{app_with, load_entries, run_cmds};
use super::*;
use crate::runtime::Msg;

#[test]
fn a_failed_placeholder_is_not_promotable_and_enter_posts_the_real_error() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/x.bin"), &[0xFF, 0xFE, 0x00])
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["x.bin"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let placeholder = app.explorer.preview.expect("placeholder minted");
    assert!(
        !crate::messages::is_open(&app),
        "no banner from the preview path itself"
    );
    app.set_focus_pane(Pane::Explorer, &mut effects);

    let enter = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Enter,
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(&mut app, Msg::Key(enter), &mut effects);

    assert!(
        app.doc(placeholder)
            .is_none_or(|doc| doc.read_only == ReadOnly::Preview),
        "Enter must not promote the placeholder in place"
    );
    assert!(
        crate::messages::is_open(&app),
        "the ordinary open path posts the real read failure loudly"
    );
    assert!(
        crate::messages::log_text(&app).contains("x.bin"),
        "the real error names the file"
    );
}

#[test]
fn moving_off_a_failed_preview_and_back_dedupes_then_retries_exactly_once() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/bad.bin"), &[0xFF, 0xFE, 0x00])
        .unwrap();
    mem.save_atomic(std::path::Path::new("/root/b.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["b.md", "bad.bin"]);
    let mut effects = Effects::default();

    app.explorer.nav.move_by(2, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    assert_eq!(effects.cmds.len(), 1, "the first visit reads once");
    run_cmds(&mut app, &mut effects);
    assert_eq!(
        app.explorer.preview_failed,
        Some(PathBuf::from("/root/bad.bin"))
    );

    after_cursor_move(&mut app, &mut effects);
    assert!(
        effects.cmds.is_empty(),
        "sitting on the same failed entry must not re-read"
    );

    app.explorer.nav.move_by(-1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    assert!(
        app.explorer.preview_failed.is_none(),
        "moving away clears it"
    );

    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    assert_eq!(
        effects.cmds.len(),
        1,
        "returning to the failed entry retries exactly once"
    );
}

#[test]
fn a_failed_preview_at_the_tab_limit_mints_no_placeholder_and_does_not_reread() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/bad.bin"), &[0xFF, 0xFE, 0x00])
        .unwrap();
    let mut app = app_with(&mem);
    for i in 0..crate::opentabs::limit::MAX_TABS {
        let id = app.open_document(CoreBuffer::new("hello"));
        app.doc_mut(id)
            .unwrap()
            .bind_path(PathBuf::from(format!("/root/doc{i}.md")));
    }
    load_entries(&mut app, &["bad.bin"]);
    let mut effects = Effects::default();
    let docs_before = app.documents.len();

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // ".." -> "bad.bin"
    after_cursor_move(&mut app, &mut effects);
    assert_eq!(effects.cmds.len(), 1, "the first visit reads once");
    run_cmds(&mut app, &mut effects);

    assert!(
        app.explorer.preview.is_none(),
        "a full tab strip must not mint a placeholder document"
    );
    assert_eq!(
        app.documents.len(),
        docs_before,
        "no document was minted for the failed preview"
    );
    assert_eq!(
        app.explorer.preview_failed,
        Some(PathBuf::from("/root/bad.bin")),
        "the failure must still be recorded even without a placeholder"
    );

    after_cursor_move(&mut app, &mut effects);
    assert!(
        effects.cmds.is_empty(),
        "sitting on the same failed entry must not re-read, even at the tab limit"
    );
}

#[test]
fn a_stale_err_reply_for_a_path_the_cursor_has_left_is_ignored() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.bin"), &[0xFF, 0xFE, 0x00])
        .unwrap();
    mem.save_atomic(std::path::Path::new("/root/b.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.bin", "b.md"]);
    let mut effects = Effects::default();

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "a.bin"
    after_cursor_move(&mut app, &mut effects);
    let stale_cmd = effects.cmds.pop().expect("a.bin read queued");

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "b.md"
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects); // resolves "b.md" first

    let shown_after_fresh = app
        .explorer
        .preview
        .and_then(|id| app.doc(id))
        .and_then(|d| d.file_path.clone());
    assert_eq!(shown_after_fresh, Some(PathBuf::from("/root/b.md")));

    if let Some(Msg::FileOpened {
        path,
        result,
        anchor,
    }) = stale_cmd.run()
    {
        workspace::handle_file_opened(&mut app, &path, result, anchor, &mut effects);
    }

    let shown_after_stale = app
        .explorer
        .preview
        .and_then(|id| app.doc(id))
        .and_then(|d| d.file_path.clone());
    assert_eq!(
        shown_after_stale,
        Some(PathBuf::from("/root/b.md")),
        "a stale Err reply must not override the fresh preview"
    );
    assert!(
        !crate::messages::is_open(&app),
        "a stale Err reply is dropped silently"
    );
}
