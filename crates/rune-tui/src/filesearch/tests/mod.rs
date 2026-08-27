#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use rune_core::buffer::Buffer;
use rune_vfs::Mem;
use std::sync::Arc;

fn app() -> App {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    app.frame = Some(crate::app::FrameSize::new(120, 34));
    app
}

#[test]
fn open_on_a_never_shown_left_column_still_opens_and_focuses_explorer() {
    let mut app = app();
    assert!(!app.splits.left.is_shown(), "test setup: column hidden");
    let mut effects = Effects::default();

    open(&mut app, &mut effects);

    assert!(app.filesearch().is_some());
    assert_eq!(app.focus(), Pane::Explorer);
}

#[test]
fn open_is_a_no_op_when_already_open() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation_before = app.filesearch().map(|s| s.generation);

    open(&mut app, &mut effects);

    assert_eq!(app.filesearch().map(|s| s.generation), generation_before);
}

#[test]
fn cancel_restores_the_document_that_was_active_before_open() {
    let mut app = app();
    let second = app.open_document(Buffer::new("second"));
    crate::workspace::switch_to(&mut app, second);
    let mut effects = Effects::default();

    open(&mut app, &mut effects);
    cancel(&mut app, &mut effects);

    assert!(app.filesearch().is_none());
    assert_eq!(
        app.active, second,
        "return_to is the doc active at open time"
    );
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn cancel_falls_back_to_the_editor_when_return_to_no_longer_exists() {
    let mut app = app();
    let second = app.open_document(Buffer::new("second"));
    crate::workspace::switch_to(&mut app, second);
    let mut effects = Effects::default();

    open(&mut app, &mut effects);
    let return_to = app.filesearch().map(|s| s.return_to).expect("open");
    assert_eq!(
        return_to,
        crate::returnto::ReturnTo::to(second),
        "test setup: return_to names `second`"
    );

    crate::workspace::request_close(&mut app, second, &mut effects);
    assert!(app.doc(second).is_none(), "test setup: closed for real");

    cancel(&mut app, &mut effects);

    assert!(app.filesearch().is_none());
    assert_eq!(app.focus(), Pane::Editor);
}

fn candidate(path: &str, in_tree: bool) -> Candidate {
    Candidate {
        path: PathBuf::from(path),
        display: path.to_string(),
        in_tree,
        mru_rank: None,
    }
}

#[test]
fn recents_loaded_orders_in_tree_before_out_of_tree_preserving_mru() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;

    let recents = vec![
        candidate("/outside/z.md", false),
        candidate("/root/a.md", true),
        candidate("/root/b.md", true),
    ];

    handle_recents_loaded(&mut app, generation, Ok(recents), &mut effects);

    let state = app.filesearch().expect("still open");
    let ordered: Vec<&std::path::Path> = state
        .results
        .iter()
        .map(|row| {
            candidate_at(state, row.candidate_idx)
                .expect("row names a real candidate")
                .path
                .as_path()
        })
        .collect();
    assert_eq!(
        ordered,
        vec![
            std::path::Path::new("/root/a.md"),
            std::path::Path::new("/root/b.md"),
            std::path::Path::new("/outside/z.md"),
        ]
    );
}

#[test]
fn recents_loaded_reply_is_dropped_after_a_close_then_reopen_mints_a_new_generation() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let stale_generation = app.filesearch().expect("open").generation;
    close(&mut app);
    open(&mut app, &mut effects);
    let fresh_generation = app.filesearch().expect("reopen").generation;
    assert_ne!(
        stale_generation, fresh_generation,
        "test setup: reopen must mint a new generation"
    );

    handle_recents_loaded(
        &mut app,
        stale_generation,
        Ok(vec![candidate("/root/a.md", true)]),
        &mut effects,
    );

    assert!(
        app.filesearch().expect("still open").recents.is_empty(),
        "a stale reply must never populate the fresh session's recents"
    );
}

#[test]
fn recents_loaded_err_reply_posts_a_message_and_leaves_recents_empty() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;

    handle_recents_loaded(
        &mut app,
        generation,
        Err(CmdError::Refused("boom".to_string())),
        &mut effects,
    );

    assert!(app.filesearch().expect("still open").recents.is_empty());
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("recent files not loaded: boom")
    );
}

#[test]
fn open_pushes_the_walk_cmd_and_marks_it_pending() {
    let mut app = app();
    app.root = Some(PathBuf::from("/root"));
    let mut effects = Effects::default();

    open(&mut app, &mut effects);

    assert!(
        app.filesearch().is_some_and(|s| s.walk_pending),
        "walk_pending is set the moment the Cmd is issued"
    );
    assert!(
        effects
            .cmds
            .iter()
            .any(|c| c.kind() == crate::runtime::CmdKind::ReadDir),
        "the scan Cmd is pushed, never run inline"
    );
}

#[test]
fn handle_scanned_drops_a_reply_whose_generation_no_longer_matches() {
    let mut app = app();
    app.root = Some(PathBuf::from("/root"));
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let stale_generation = app.filesearch().expect("open").generation;
    cancel(&mut app, &mut effects);
    open(&mut app, &mut effects);

    handle_scanned(
        &mut app,
        stale_generation,
        Ok(walk::ScanResult {
            files: vec![PathBuf::from("/root/a.md")],
            truncated: false,
        }),
        &mut effects,
    );

    assert!(
        app.filesearch().is_some_and(|s| s.walk.is_empty()),
        "a stale reply must never populate the live session's walk results"
    );
    assert!(
        app.filesearch().is_some_and(|s| s.walk_pending),
        "the live session's own still-in-flight scan is untouched"
    );
}

mod more;
