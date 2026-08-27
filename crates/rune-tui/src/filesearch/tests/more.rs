use super::*;

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
    app.explorer.preview_awaiting = None;

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

fn dedup_test_recents() -> Vec<Candidate> {
    vec![Candidate {
        path: PathBuf::from("/root/a.md"),
        display: "a.md".to_string(),
        in_tree: true,
        mru_rank: Some(0),
    }]
}

fn dedup_test_scan() -> walk::ScanResult {
    walk::ScanResult {
        files: vec![PathBuf::from("/root/a.md"), PathBuf::from("/root/b.md")],
        truncated: false,
    }
}

fn listed_paths(app: &App) -> Vec<PathBuf> {
    let state = app.filesearch().expect("still open");
    state
        .results
        .iter()
        .filter_map(|r| candidate_at(state, r.candidate_idx))
        .map(|c| c.path.clone())
        .collect()
}

/// Defect 2: the finder's recents load and its workspace walk are two
/// independent one-shot replies that can land in either order. Whichever
/// order they land in, `/root/a.md` — named by both — must be listed
/// exactly once, never twice, and the two orders must produce the identical
/// row set.
#[test]
fn recents_then_scan_and_scan_then_recents_produce_identical_deduped_rows() {
    let mut effects = Effects::default();

    let mut recents_first = app();
    recents_first.root = Some(PathBuf::from("/root"));
    open(&mut recents_first, &mut effects);
    let generation = recents_first.filesearch().expect("open").generation;
    handle_recents_loaded(
        &mut recents_first,
        generation,
        Ok(dedup_test_recents()),
        &mut effects,
    );
    handle_scanned(
        &mut recents_first,
        generation,
        Ok(dedup_test_scan()),
        &mut effects,
    );

    let mut scan_first = app();
    scan_first.root = Some(PathBuf::from("/root"));
    open(&mut scan_first, &mut effects);
    let generation = scan_first.filesearch().expect("open").generation;
    handle_scanned(
        &mut scan_first,
        generation,
        Ok(dedup_test_scan()),
        &mut effects,
    );
    handle_recents_loaded(
        &mut scan_first,
        generation,
        Ok(dedup_test_recents()),
        &mut effects,
    );

    let recents_first_paths = listed_paths(&recents_first);
    let scan_first_paths = listed_paths(&scan_first);

    let expected = vec![PathBuf::from("/root/a.md"), PathBuf::from("/root/b.md")];
    assert_eq!(
        recents_first_paths, expected,
        "recents-then-scan must list each path exactly once"
    );
    assert_eq!(
        scan_first_paths, expected,
        "scan-then-recents must produce the identical deduped rows, \
         regardless of which reply landed first"
    );
}
