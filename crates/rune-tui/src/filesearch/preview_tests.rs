#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, Vfs, VfsTestExt};

use super::*;
use crate::document::ReadOnly;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::runtime::{CmdKind, Msg};

fn seeded_app(files: &[(&str, &str)]) -> App {
    let mem = Mem::new();
    for (path, content) in files {
        mem.save_atomic(std::path::Path::new(path), content.as_bytes())
            .expect("seed file");
    }
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(mem), None);
    app.frame = Some(crate::app::FrameSize::new(120, 34));
    app.root = Some(PathBuf::from("/root"));
    app
}

fn candidate(path: &str) -> Candidate {
    Candidate {
        path: PathBuf::from(path),
        display: path.trim_start_matches("/root/").to_string(),
        in_tree: true,
        mru_rank: None,
    }
}

fn run_cmds(app: &mut App, effects: &mut Effects) {
    let cmds = std::mem::take(&mut effects.cmds);
    for cmd in cmds {
        if let Some(Msg::FileOpened {
            path,
            result,
            anchor,
            preview_generation,
        }) = cmd.run()
        {
            crate::workspace::handle_file_opened(
                app,
                &path,
                result,
                anchor,
                preview_generation,
                effects,
            );
        }
    }
}

fn down_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Down,
        mods: Mods::NONE,
    }
}

fn escape_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Escape,
        mods: Mods::NONE,
    }
}

fn enter_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Enter,
        mods: Mods::NONE,
    }
}

#[test]
fn cursor_move_onto_an_unopened_candidate_queues_a_read_file_cmd() {
    let mut app = seeded_app(&[("/root/a.md", "a"), ("/root/b.md", "b")]);
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![candidate("/root/a.md"), candidate("/root/b.md")]),
        &mut effects,
    );
    effects.cmds.clear();

    let _ = keys::handle_key(&mut app, down_key(), &mut effects);

    assert!(
        effects.cmds.iter().any(|c| c.kind() == CmdKind::ReadFile),
        "moving onto b.md must queue its own preview read"
    );
}

#[test]
fn hand_delivered_preview_reply_lands_in_the_explorers_preview_slot() {
    let mut app = seeded_app(&[("/root/a.md", "content")]);
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;

    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![candidate("/root/a.md")]),
        &mut effects,
    );
    run_cmds(&mut app, &mut effects);

    assert_eq!(
        crate::explorer_preview::shown_path(&app),
        Some(std::path::Path::new("/root/a.md"))
    );
    assert_eq!(
        app.documents.order().len(),
        1,
        "a preview opens no tab of its own"
    );
}

#[test]
fn a_stale_reply_for_a_path_the_cursor_left_is_dropped() {
    let mut app = seeded_app(&[("/root/a.md", "a"), ("/root/b.md", "b")]);
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![candidate("/root/a.md"), candidate("/root/b.md")]),
        &mut effects,
    );
    let stale_cmd = effects.cmds.pop().expect("a.md's own preview Cmd queued");

    let _ = keys::handle_key(&mut app, down_key(), &mut effects);
    run_cmds(&mut app, &mut effects);

    assert_eq!(
        crate::explorer_preview::shown_path(&app),
        Some(std::path::Path::new("/root/b.md"))
    );

    if let Some(Msg::FileOpened {
        path,
        result,
        anchor,
        preview_generation,
    }) = stale_cmd.run()
    {
        crate::workspace::handle_file_opened(
            &mut app,
            &path,
            result,
            anchor,
            preview_generation,
            &mut effects,
        );
    }
    assert_eq!(
        crate::explorer_preview::shown_path(&app),
        Some(std::path::Path::new("/root/b.md")),
        "the late a.md reply must not override the b.md preview already showing"
    );
}

#[test]
fn enter_on_a_previewed_row_promotes_rather_than_reopening() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    vfs.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .expect("seed a.md");
    let clock: rune_db::ClockFn = Arc::new(std::time::SystemTime::now);
    let store =
        rune_db::Store::open_in_memory(clock, Arc::clone(&vfs), Box::new(|_evt| {})).expect("db");
    let bridge = crate::db::DbBridge::bootstrap();
    let mut app = App::new(Buffer::new("hello"), None, vfs, None);
    app.frame = Some(crate::app::FrameSize::new(120, 34));
    app.root = Some(PathBuf::from("/root"));
    app.db = Some(crate::db::Db::new(store, bridge, false));
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![candidate("/root/a.md")]),
        &mut effects,
    );
    run_cmds(&mut app, &mut effects);
    let id = app.explorer.preview.as_ref().expect("preview minted").id;
    let tabs_before = app.documents.order().len();

    let _ = keys::handle_key(&mut app, enter_key(), &mut effects);

    assert!(app.filesearch().is_none());
    assert_eq!(app.doc(id).expect("doc").read_only, ReadOnly::No);
    assert!(app.explorer.preview.is_none());
    assert_eq!(app.active, id, "the promoted document becomes active");
    assert_eq!(
        app.documents.order().len(),
        tabs_before + 1,
        "promotion opens the tab the preview never held"
    );
    assert!(
        app.db_ops
            .values()
            .any(|op| op.doc == id && op.issued_version.is_some()),
        "promotion must enqueue recovery-store hydration"
    );
}

#[test]
fn esc_after_arrowing_onto_an_already_open_document_restores_return_to() {
    let mut app = seeded_app(&[("/root/a.md", "a"), ("/root/b.md", "b")]);
    let return_to = app.active;
    let opened = crate::workspace::open_path(&mut app, std::path::Path::new("/root/b.md"))
        .expect("open b.md as a real tab");
    crate::workspace::switch_to(&mut app, return_to);
    assert_eq!(app.active, return_to, "test setup: parked back off b.md");
    let mut effects = Effects::default();

    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![candidate("/root/b.md")]),
        &mut effects,
    );

    assert_eq!(app.active, opened, "test setup: cursor landed on b.md");
    assert!(
        app.explorer.preview.is_none(),
        "test setup: an already-open document mints no preview"
    );

    let _ = keys::handle_key(&mut app, escape_key(), &mut effects);

    assert!(app.filesearch().is_none());
    assert_eq!(app.active, return_to);
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn esc_after_previewing_a_not_open_file_restores_return_to_and_discards_the_preview() {
    let mut app = seeded_app(&[("/root/a.md", "content")]);
    let return_to = app.active;
    let mut effects = Effects::default();

    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![candidate("/root/a.md")]),
        &mut effects,
    );
    run_cmds(&mut app, &mut effects);
    assert!(
        app.explorer.preview.is_some(),
        "test setup: a real preview was minted"
    );
    assert_eq!(
        app.active, return_to,
        "test setup: previewing never moves the caret off the real document"
    );

    let _ = keys::handle_key(&mut app, escape_key(), &mut effects);

    assert!(app.filesearch().is_none());
    assert_eq!(app.active, return_to);
    assert_eq!(app.focus(), Pane::Editor);
    assert!(
        app.explorer.preview.is_none(),
        "the discarded preview must not survive Esc"
    );
}
