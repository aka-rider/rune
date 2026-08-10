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

/// Drains and runs every queued `Cmd` through the real `app::update`
/// chokepoint, unlike [`run_cmds`] above which calls straight into
/// `workspace::handle_file_opened` and so never reaches `dispatch::
/// after_update`'s highlight-reschedule check. Needed for any assertion
/// about scheduling, not just document/tab bookkeeping.
fn run_cmds_through_update(app: &mut App, effects: &mut Effects) {
    let cmds = std::mem::take(&mut effects.cmds);
    for cmd in cmds {
        if let Some(msg) = cmd.run() {
            crate::app::update(app, msg, effects);
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

    let id = app.explorer.preview.expect("a placeholder is minted");
    let doc = app.doc(id).expect("placeholder document exists");
    assert!(doc.buffer.content().contains("huge.md"));
    assert!(doc.buffer.content().contains("too large to preview"));
    assert_eq!(doc.read_only, ReadOnly::Preview);
    assert!(doc.file_path.is_none(), "a placeholder is never bound");
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

    let id = app.explorer.preview.expect("a placeholder is minted");
    let doc = app.doc(id).expect("placeholder document exists");
    assert!(doc.buffer.content().contains("x.bin"));
    assert!(doc.buffer.content().contains("not valid UTF-8"));
    assert_eq!(doc.read_only, ReadOnly::Preview);
    assert!(doc.file_path.is_none(), "a placeholder is never bound");
    assert!(
        !crate::messages::is_open(&app),
        "no message pane for a failed preview"
    );
}

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

    app.explorer.nav.move_by(2, app.explorer.entries.len()); // ".." -> "b.md" -> "bad.bin"
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

    app.explorer.nav.move_by(-1, app.explorer.entries.len()); // "b.md"
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    assert!(
        app.explorer.preview_failed.is_none(),
        "moving away clears it"
    );

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // back to "bad.bin"
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
        workspace::handle_file_opened(&mut app, path, result, anchor, &mut effects);
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

/// `^e` (`GlobalCommand::FocusEditor`) no longer exists in the shipped
/// keymap — the keys half of this merge deleted it. Escape from the
/// Explorer (`ExplorerCommand::Leave`) is chosen as its replacement route
/// over `^B`'s hide branch: both are pure focus transitions that reach
/// `on_focus_changed` with no document switch of their own, but Escape is
/// the one a user actually presses right after arrowing the Explorer (the
/// same gesture that minted the preview in the first place), so it
/// exercises the promote hook against the exact sequence the feature is
/// for. `^B`'s hide branch gets its own dedicated coverage below.
#[test]
fn escape_from_the_explorer_promotes_the_live_preview() {
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
    // `on_focus_changed` only reacts to an actual TRANSITION — land on the
    // Explorer first (browsing it is what minted the preview above in the
    // first place).
    app.set_focus_pane(Pane::Explorer, &mut effects);

    let escape = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Escape,
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(&mut app, Msg::Key(escape), &mut effects);

    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.doc(id).unwrap().read_only, ReadOnly::No);
    assert!(app.explorer.preview.is_none(), "preview slot cleared");
    assert_eq!(
        app.tabs.order.iter().filter(|&&t| t == id).count(),
        1,
        "promoted document appears exactly once in tabs.order"
    );
    assert_eq!(
        app.tabs.order.len(),
        tabs_before,
        "promotion mints no extra tab"
    );
}

/// `^B` (`GlobalCommand::ToggleLeft`)'s hide branch: painted this frame ⇒
/// hides the column and hands focus to the Editor. That's the second, and
/// last, pure-focus route into the Editor the shipped keymap has —
/// `pane::handle_global_command`'s `ToggleLeft` arm calls `set_focus_pane`
/// directly, touching no document, exactly like Escape above.
#[test]
fn ctrl_b_hiding_the_column_promotes_the_live_preview() {
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
    app.set_focus_pane(Pane::Explorer, &mut effects);
    assert!(app.splits.left.is_shown(), "column starts visible");

    let ctrl_b = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char('b'),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    };
    crate::app::update(&mut app, Msg::Key(ctrl_b), &mut effects);

    assert!(!app.splits.left.is_shown(), "the column collapses");
    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.doc(id).unwrap().read_only, ReadOnly::No);
    assert!(app.explorer.preview.is_none());
}

/// Enter on a file row promotes through `explorer_keys::open_selected`'s own
/// direct call to `explorer_preview::promote` — not through
/// `on_focus_changed`'s focus-transition hook, though that hook also fires
/// (harmlessly, as a no-op — the preview slot is already clear by the time
/// it runs) since `Open` moves focus to the Editor too. Driven through a
/// real `Enter` key message rather than calling `promote` directly, so this
/// proves the whole route the keymap actually offers, not just the
/// function in isolation.
#[test]
fn enter_on_a_file_row_promotes_the_preview_via_the_direct_call_path() {
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
    app.set_focus_pane(Pane::Explorer, &mut effects);

    let enter = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Enter,
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(&mut app, Msg::Key(enter), &mut effects);

    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.doc(id).unwrap().read_only, ReadOnly::No);
    assert!(app.explorer.preview.is_none());
    assert_eq!(
        app.tabs.order.iter().filter(|&&t| t == id).count(),
        1,
        "exactly one tab for the promoted document"
    );
    assert_eq!(app.tabs.order.len(), tabs_before, "no second tab appears");
}

/// Escape from the Tabs pane also lands on the Editor — the same
/// `on_focus_changed` `Pane::Editor` arm Escape-from-Explorer and `^B`'s
/// hide branch reach. The difference: reaching the Tabs pane at all
/// (`^t`, `GlobalCommand::FocusTabs`) is itself a focus transition that
/// `on_focus_changed`'s `Pane::Title | Pane::Tabs` arm discards the live
/// preview for, in the SAME `app::update` call that moved focus there — so
/// by the time a later, separate Escape keypress reaches the Tabs pane, no
/// preview is left to promote. This pins that the code does NOT double-fire
/// a promote against an already-discarded preview, and leaves no dangling
/// document behind either.
#[test]
fn escape_from_the_tabs_pane_has_no_preview_left_to_promote() {
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
    app.set_focus_pane(Pane::Explorer, &mut effects);

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
    assert!(
        app.explorer.preview.is_none(),
        "landing on Tabs already discarded it"
    );
    assert!(app.doc(preview_id).is_none());

    let escape = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Escape,
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(&mut app, Msg::Key(escape), &mut effects);

    assert_eq!(app.focus(), Pane::Editor);
    assert!(app.explorer.preview.is_none());
    assert!(
        !app.tabs.order.contains(&preview_id),
        "the discarded preview never reappears"
    );
    assert_eq!(app.active, real, "the surviving real tab stays active");
}

/// A live in-memory `Db` — mirrors `pane.rs::tests::live_db` — needed to
/// prove `promote` actually enqueues recovery-store hydration (`App::db_ops`
/// gains a `Load` entry for the promoted document) rather than merely
/// flipping `read_only`. Every other test in this module runs with
/// `app.db == None`, where `db_enqueue::load_document` is a documented
/// no-op, so none of them could catch a promote that stopped hydrating.
fn live_db() -> crate::db::Db {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: rune_db::ClockFn = Arc::new(std::time::SystemTime::now);
    let store =
        rune_db::Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
    let bridge = crate::db::DbBridge::bootstrap();
    crate::db::Db::new(store, bridge, false)
}

#[test]
fn promoting_the_preview_enqueues_recovery_store_hydration() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    app.db = Some(live_db());
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let id = app.explorer.preview.expect("preview minted");
    app.set_focus_pane(Pane::Explorer, &mut effects);
    assert!(
        app.db_ops.is_empty(),
        "a preview never contacts the recovery store before promotion"
    );

    let escape = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Escape,
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(&mut app, Msg::Key(escape), &mut effects);

    assert!(
        app.db_ops
            .values()
            .any(|op| op.doc == id && op.issued_version.is_some()),
        "promotion must enqueue a Load op hydrating the document through the recovery store"
    );
}

/// Finding 1 (HIGH): `Document::new` resets `highlight` and resets the
/// swapped-in buffer to version 1 — and a preview's buffer is never
/// edited, so before `apply_loaded` advanced the version past the reused
/// document's own, every preview after the first sat at version 1 forever.
/// `dispatch::after_update`'s highlight-reschedule check is gated on the
/// buffer version actually changing, so nothing was ever scheduled past
/// the first file arrowed onto — this pins that a highlight is scheduled
/// for EVERY file in the run, not just the first.
#[test]
fn arrowing_across_files_schedules_a_highlight_for_every_preview() {
    let mem = Arc::new(Mem::new());
    let code = "```rust\nfn a() {}\n```\n";
    for name in ["a.md", "b.md", "c.md"] {
        mem.save_atomic(&PathBuf::from("/root").join(name), code.as_bytes())
            .unwrap();
    }
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md", "b.md", "c.md"]);
    let mut effects = Effects::default();

    for n in 0..3 {
        app.explorer.nav.move_by(1, app.explorer.entries.len());
        after_cursor_move(&mut app, &mut effects);
        run_cmds_through_update(&mut app, &mut effects);

        let id = app.explorer.preview.expect("preview minted");
        assert!(
            app.doc(id).expect("doc").highlight.in_flight.is_some(),
            "file #{n} previewed onto the reused document must schedule its own highlight"
        );
    }
}

/// Finding 1 (HIGH): the in-place swap's stale-reply hazard, constructed
/// directly rather than raced across threads — the danger is a version
/// COLLISION (both buffers independently starting at version 1 under the
/// same reused id), not a timing window, so feeding a direct `Msg::
/// Highlighted` at the version captured just before the swap exercises
/// exactly the check a genuinely late reply would hit. Before `apply_
/// loaded` advanced the version, this reply's `version` would have equalled
/// the new file's live version by coincidence and `dispatch::
/// handle_highlighted` would have installed file A's regions onto file B.
#[test]
fn a_highlight_reply_for_the_previous_preview_file_cannot_paint_the_next_one() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"# a\n")
        .unwrap();
    mem.save_atomic(std::path::Path::new("/root/b.md"), b"# b\n")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md", "b.md"]);
    let mut effects = Effects::default();

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "a.md"
    after_cursor_move(&mut app, &mut effects);
    run_cmds_through_update(&mut app, &mut effects);
    let id = app.explorer.preview.expect("preview minted");
    let stale_version = app.doc(id).expect("doc").buffer.version();

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "b.md"
    after_cursor_move(&mut app, &mut effects);
    run_cmds_through_update(&mut app, &mut effects);
    assert_eq!(app.explorer.preview, Some(id), "the same id is reused");
    let live_version = app.doc(id).expect("doc").buffer.version();
    assert_ne!(
        stale_version, live_version,
        "the swap must advance the buffer's version"
    );

    let scope = rune_syntax::scope::scope_table()
        .resolve("keyword")
        .expect("known scope");
    let marker_line = 0..4;
    crate::app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version: stale_version,
            result: Some(crate::highlight::HighlightReply {
                regions: vec![crate::highlight::RegionResult {
                    map: crate::linemap::LineMap::new(vec![marker_line]),
                    payload: Some(crate::highlight::RegionPayload::Spans(vec![(0..4, scope)])),
                }],
                truncated: false,
            }),
        },
        &mut effects,
    );

    let doc = app.doc(id).expect("doc");
    assert!(
        doc.highlight.regions.is_empty(),
        "a reply computed for the file this id used to show must never install \
         onto the file that replaced it"
    );
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

/// Finding 2 (MEDIUM): with several real tabs open and a NON-FIRST one
/// active, discarding a preview via `^t` (`GlobalCommand::FocusTabs`,
/// routed through `on_focus_changed`'s `Pane::Tabs` arm) must restore the
/// document the user was actually editing before browsing — not
/// `tabs.order.first()`, which this test's setup deliberately makes a
/// DIFFERENT document so the old tab-0 fallback would be caught.
#[test]
fn discarding_a_preview_via_ctrl_t_restores_the_document_active_before_previewing() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    app.open_document(CoreBuffer::new("second"));
    let third = app.open_document(CoreBuffer::new("third"));
    workspace::switch_to(&mut app, third);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let preview_id = app.explorer.preview.expect("preview minted");
    assert_eq!(
        app.active, preview_id,
        "browsing lands on the preview itself"
    );
    assert_ne!(
        app.tabs.order.first().copied(),
        Some(third),
        "tab 0 must NOT be the document that was active before previewing, \
         or this test could not tell the fix from the old tab-0 fallback"
    );

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

    assert!(app.doc(preview_id).is_none());
    assert_eq!(
        app.active, third,
        "must restore the document active before previewing, not tab 0"
    );
}

/// Finding 2 (MEDIUM): the same restoration, reached through focus landing
/// on the Title pane (`^r`, `GlobalCommand::FocusTitle`) instead of Tabs —
/// `on_focus_changed`'s `Pane::Title | Pane::Tabs` arm shares one
/// `discard_active` for both, so this pins the other half of that match.
///
/// Drives `on_focus_changed` directly rather than through `^r`
/// (`GlobalCommand::FocusTitle`): `focus_title` itself refuses whenever the
/// ACTIVE document is read-only, and the previewed document (`ReadOnly::
/// Preview`) is always active while browsing — so this exact transition has
/// no reachable keymap route today, only whatever future path (a mouse
/// click on the Title bar) lands focus there directly the way `set_focus_
/// pane` does.
#[test]
fn discarding_a_preview_via_focus_to_title_restores_the_document_active_before_previewing() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    app.open_document(CoreBuffer::new("second"));
    let third = app.open_document(CoreBuffer::new("third"));
    workspace::switch_to(&mut app, third);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let preview_id = app.explorer.preview.expect("preview minted");

    on_focus_changed(&mut app, Pane::Explorer, Pane::Title);

    assert!(app.doc(preview_id).is_none());
    assert_eq!(
        app.active, third,
        "must restore the document active before previewing, not tab 0"
    );
}

/// Finding 2 (MEDIUM) control case: selecting a specific tab by digit
/// while previewing (`^N`, `GlobalCommand::TabSwitch`) reaches
/// `discard_if_switching_away` — a DIFFERENT route than `discard_active`,
/// already correct before this fix — and lands on exactly the tab the
/// digit named. Pinned here so the same scenario (several tabs, a
/// non-first one active before browsing) is covered across every discard
/// route the finding calls out, not only the two `discard_active` used.
#[test]
fn discarding_a_preview_via_ctrl_digit_selects_exactly_the_named_tab() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    app.open_document(CoreBuffer::new("second"));
    let third = app.open_document(CoreBuffer::new("third"));
    workspace::switch_to(&mut app, third);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let preview_id = app.explorer.preview.expect("preview minted");
    let third_index = app
        .tabs
        .order
        .iter()
        .position(|&t| t == third)
        .expect("third tab is open");

    let ctrl_digit = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char(
            char::from_digit((third_index as u32 + 1) % 10, 10).expect("valid digit"),
        ),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    };
    crate::app::update(&mut app, Msg::Key(ctrl_digit), &mut effects);

    assert!(app.doc(preview_id).is_none());
    assert_eq!(app.active, third, "selecting the tab by digit lands on it");
}

/// Finding 2 (MEDIUM) fallback: the remembered pre-preview document can
/// itself be closed while browsing is still live (e.g. via the Tabs
/// pane's own `^w`). `discard_active` must then fall back to
/// `workspace::close::neighbor_of`'s adjacent-tab pick — reused rather
/// than a second neighbour picker — instead of leaving `app.active`
/// pointed at a document that no longer exists.
#[test]
fn discarding_a_preview_falls_back_to_the_adjacent_tab_when_the_remembered_document_was_closed() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    let second = app.open_document(CoreBuffer::new("second"));
    let third = app.open_document(CoreBuffer::new("third"));
    workspace::switch_to(&mut app, third);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let preview_id = app.explorer.preview.expect("preview minted");
    assert_eq!(app.explorer.preview_return_to, Some(third));

    // `third` — the remembered return-to document — closes while the
    // preview is still live, e.g. via the Tabs pane.
    let _ = workspace::close_now(&mut app, third, &mut effects);
    assert!(app.doc(third).is_none());

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

    assert!(app.doc(preview_id).is_none());
    assert!(app.doc(third).is_none(), "the closed document stays closed");
    assert!(
        app.doc(second).is_some() && app.active == second,
        "falls back to the surviving neighbour, not the stale remembered document"
    );
}
