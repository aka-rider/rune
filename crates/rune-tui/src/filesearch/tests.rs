#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use rune_core::buffer::Buffer;
use rune_vfs::Mem;
use std::sync::Arc;

fn app() -> App {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    app.frame_width = 120;
    app.frame_height = 34;
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

#[test]
fn handle_scanned_dedups_walk_against_recents_keeping_the_recents_mru_rank() {
    let mut app = app();
    app.root = Some(PathBuf::from("/root"));
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    if let Some(state) = app.filesearch_mut() {
        state.recents.push(Candidate {
            path: PathBuf::from("/root/a.md"),
            display: "a.md".to_string(),
            in_tree: true,
            mru_rank: Some(0),
        });
    }

    handle_scanned(
        &mut app,
        generation,
        Ok(walk::ScanResult {
            files: vec![PathBuf::from("/root/a.md"), PathBuf::from("/root/b.md")],
            truncated: false,
        }),
        &mut effects,
    );

    let state = app.filesearch().expect("still open");
    assert_eq!(
        state
            .walk
            .iter()
            .map(|c| c.path.clone())
            .collect::<Vec<_>>(),
        vec![PathBuf::from("/root/b.md")],
        "the path already covered by a recent is dropped from walk, not duplicated"
    );
    assert_eq!(state.recents.first().and_then(|c| c.mru_rank), Some(0));
    assert!(!state.walk_pending);
    assert_eq!(
        state
            .results
            .first()
            .and_then(|r| candidate_at(state, r.candidate_idx)),
        state.recents.first(),
        "recents occupy the low flat indices"
    );
    assert_eq!(
        state
            .results
            .get(1)
            .and_then(|r| candidate_at(state, r.candidate_idx)),
        state.walk.first(),
        "walk results follow recents in the flat listing"
    );
}

#[test]
fn handle_scanned_error_clears_pending_and_posts_a_message() {
    let mut app = app();
    app.root = Some(PathBuf::from("/root"));
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;

    handle_scanned(
        &mut app,
        generation,
        Err("workspace root vanished".to_string()),
        &mut effects,
    );

    assert!(app.filesearch().is_some_and(|s| !s.walk_pending));
    assert!(app.filesearch().is_some_and(|s| s.walk.is_empty()));
    assert!(
        crate::messages::newest_text(&app).is_some(),
        "a message was posted"
    );
}

#[test]
fn a_basename_match_outranks_a_buried_mid_word_match() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![
            Candidate {
                path: PathBuf::from("/root/x/annotated.md"),
                display: "x/annotated.md".to_string(),
                in_tree: true,
                mru_rank: None,
            },
            Candidate {
                path: PathBuf::from("/root/y/note.md"),
                display: "y/note.md".to_string(),
                in_tree: true,
                mru_rank: None,
            },
        ]),
        &mut effects,
    );
    if let Some(state) = app.filesearch_mut() {
        state.query = "note".to_string();
    }

    recompute(&mut app, &mut effects);

    let state = app.filesearch().expect("still open");
    let top = state
        .results
        .first()
        .and_then(|r| candidate_at(state, r.candidate_idx))
        .expect("at least one match");
    assert_eq!(
        top.path,
        PathBuf::from("/root/y/note.md"),
        "the boundary basename match must rank first, got order: {:?}",
        state
            .results
            .iter()
            .filter_map(|r| candidate_at(state, r.candidate_idx))
            .map(|c| c.path.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn in_tree_beats_out_of_tree_at_equal_score() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![
            candidate("/outside/app.rs", false),
            candidate("/root/app.rs", true),
        ]),
        &mut effects,
    );
    if let Some(state) = app.filesearch_mut() {
        state.query = "app".to_string();
    }

    recompute(&mut app, &mut effects);

    let state = app.filesearch().expect("still open");
    let ordered: Vec<bool> = state
        .results
        .iter()
        .filter_map(|r| candidate_at(state, r.candidate_idx))
        .map(|c| c.in_tree)
        .collect();
    assert_eq!(
        ordered,
        vec![true, false],
        "in-tree must sort ahead of out-of-tree at equal score"
    );
}

#[test]
fn mru_rank_breaks_a_score_tie() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![
            candidate("/root/b/note.md", true),
            candidate("/root/a/note.md", true),
        ]),
        &mut effects,
    );
    if let Some(state) = app.filesearch_mut() {
        for c in &mut state.recents {
            c.display = "note.md".to_string();
        }
        state.query = "note".to_string();
    }

    recompute(&mut app, &mut effects);

    let state = app.filesearch().expect("still open");
    let top = state
        .results
        .first()
        .and_then(|r| candidate_at(state, r.candidate_idx))
        .expect("at least one match");
    assert_eq!(
        top.path,
        PathBuf::from("/root/b/note.md"),
        "the lower (more recent) mru_rank must sort first on a score tie"
    );
}

#[test]
fn a_walk_reply_landing_with_the_cursor_on_row_two_keeps_the_selection_on_the_same_path() {
    let mut app = app();
    app.root = Some(PathBuf::from("/root"));
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![
            candidate("/root/a.md", true),
            candidate("/root/b.md", true),
            candidate("/root/c.md", true),
        ]),
        &mut effects,
    );
    if let Some(state) = app.filesearch_mut() {
        state.nav.cursor = 2;
    }
    let selected_before = selected_candidate(&app).map(|c| c.path.clone());
    assert_eq!(
        selected_before,
        Some(PathBuf::from("/root/c.md")),
        "test setup: row 2 names c.md"
    );

    handle_scanned(
        &mut app,
        generation,
        Ok(walk::ScanResult {
            files: vec![PathBuf::from("/root/d.md")],
            truncated: false,
        }),
        &mut effects,
    );

    let state = app.filesearch().expect("still open");
    let selected_after = state
        .results
        .get(state.nav.cursor)
        .and_then(|row| candidate_at(state, row.candidate_idx))
        .map(|c| c.path.clone());
    assert_eq!(
        selected_after,
        Some(PathBuf::from("/root/c.md")),
        "a late walk reply must not steal the cursor off the user's own selection"
    );
}

#[test]
fn a_query_edit_recompute_resets_the_cursor_to_the_top() {
    let mut app = app();
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![
            candidate("/root/a.md", true),
            candidate("/root/b.md", true),
        ]),
        &mut effects,
    );
    if let Some(state) = app.filesearch_mut() {
        state.nav.cursor = 1;
        state.query = "b".to_string();
    }

    reset_and_recompute(&mut app, &mut effects);

    let state = app.filesearch().expect("still open");
    assert_eq!(
        state.nav.cursor, 0,
        "a query edit always resets the cursor to the top of the freshly filtered list"
    );
}

#[test]
fn two_query_edits_with_the_same_top_hit_push_exactly_one_preview_cmd_total() {
    let mut app = app();
    app.root = Some(PathBuf::from("/root"));
    let mut effects = Effects::default();
    open(&mut app, &mut effects);
    let generation = app.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut app,
        generation,
        Ok(vec![
            candidate("/root/apple.md", true),
            candidate("/root/notebook.md", true),
        ]),
        &mut effects,
    );
    effects.cmds.clear();

    if let Some(state) = app.filesearch_mut() {
        state.query = "n".to_string();
    }
    reset_and_recompute(&mut app, &mut effects);
    let read_file_cmds_after_first_edit = effects
        .cmds
        .iter()
        .filter(|c| c.kind() == crate::runtime::CmdKind::ReadFile)
        .count();
    assert_eq!(
        read_file_cmds_after_first_edit, 1,
        "the top hit changed (apple.md -> notebook.md), so exactly one preview read is queued"
    );
    effects.cmds.clear();
    app.explorer
        .preview_awaiting
        .remove(std::path::Path::new("/root/notebook.md"));

    if let Some(state) = app.filesearch_mut() {
        state.query = "no".to_string();
    }
    reset_and_recompute(&mut app, &mut effects);

    assert!(
        effects
            .cmds
            .iter()
            .all(|c| c.kind() != crate::runtime::CmdKind::ReadFile),
        "the top hit did not change between the two edits (still notebook.md), \
         so no second preview read must be queued"
    );
}
