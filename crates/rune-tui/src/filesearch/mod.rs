use std::path::{Path, PathBuf};
use std::sync::Arc;

use nucleo_matcher::{Config, Matcher};

use crate::app::App;
use crate::commands::mouse::WHEEL_ROWS;
use crate::listnav;
use crate::pane::Pane;
use crate::pointer::{MouseInput, MouseKind};
use crate::runtime::{CmdError, Effects};
use crate::workspace;

pub(crate) mod keys;
mod rank;
pub(crate) mod walk;

#[cfg(test)]
mod preview_tests;
#[cfg(test)]
mod tests;

const RESULT_CAP: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub display: String,
    pub in_tree: bool,
    pub mru_rank: Option<usize>,
}

pub struct ResultRow {
    pub candidate_idx: usize,
    pub indices: Vec<u32>,
}

pub struct FileSearchState {
    pub query: String,
    pub nav: listnav::List,
    pub generation: crate::generation::FileSearchGen,
    pub return_to: crate::returnto::ReturnTo,
    pub root: PathBuf,
    pub recents: Vec<Candidate>,
    pub walk: Vec<Candidate>,
    pub walk_pending: bool,
    pub walk_truncated: bool,
    pub results: Vec<ResultRow>,
    matcher: Matcher,
    charbuf: Vec<char>,
}

pub(crate) fn open(app: &mut App, effects: &mut Effects) {
    if app.filesearch().is_some() {
        return;
    }
    let Some(clearance) = app.clear_title_for_overlay(effects) else {
        return;
    };
    crate::search::close(app);
    crate::explorer_search::clear_search(app);
    let return_to = crate::returnto::ReturnTo::to(app.active);
    let generation = app.next_filesearch_gen.mint();
    let root = resolve_root(app);
    app.open_filesearch(
        FileSearchState {
            query: String::new(),
            nav: listnav::List { cursor: 0, top: 0 },
            generation,
            return_to,
            root: root.clone(),
            recents: Vec::new(),
            walk: Vec::new(),
            walk_pending: true,
            walk_truncated: false,
            results: Vec::new(),
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            charbuf: Vec::new(),
        },
        clearance,
    );
    effects.cmds.push(crate::runtime::filesearch_scan_cmd(
        Arc::clone(&app.vfs),
        root.clone(),
        generation,
    ));
    app.set_focus_pane(Pane::Explorer, effects);

    if let Some(db) = app.db.as_ref() {
        effects
            .cmds
            .push(crate::runtime::load_filesearch_recents_cmd(
                db.store.reader_query(),
                Arc::clone(&app.vfs),
                root,
                generation,
            ));
    }
}

fn resolve_root(app: &App) -> PathBuf {
    let Some(root) = app.root.as_deref() else {
        return crate::explorer::initial_root(app);
    };
    app.vfs.resolve(root).unwrap_or_else(|_| root.to_path_buf())
}

pub(crate) fn close(app: &mut App) {
    app.close_filesearch();
}

pub(crate) fn cancel(app: &mut App, effects: &mut Effects) {
    let Some(return_to) = app.filesearch().map(|s| s.return_to) else {
        return;
    };
    close(app);
    if let Some(target) = return_to.live(app) {
        workspace::switch_to(app, target);
    }
    app.set_focus_pane(Pane::Editor, effects);
}

pub(crate) fn mouse(app: &mut App, input: MouseInput, effects: &mut Effects) {
    match input.kind {
        MouseKind::ScrollUp => keys::nav_move(app, -WHEEL_ROWS, effects),
        MouseKind::ScrollDown => keys::nav_move(app, WHEEL_ROWS, effects),
        _ => {}
    }
}

fn candidate_by<'a>(
    recents: &'a [Candidate],
    walk: &'a [Candidate],
    idx: usize,
) -> Option<&'a Candidate> {
    idx.checked_sub(recents.len())
        .map_or_else(|| recents.get(idx), |walk_idx| walk.get(walk_idx))
}

pub(crate) fn candidate_at(state: &FileSearchState, idx: usize) -> Option<&Candidate> {
    candidate_by(&state.recents, &state.walk, idx)
}

pub(crate) fn selected_candidate(app: &App) -> Option<&Candidate> {
    let state = app.filesearch()?;
    let row = state.results.get(state.nav.cursor)?;
    candidate_at(state, row.candidate_idx)
}

pub(crate) fn after_cursor_move(app: &mut App, effects: &mut Effects) {
    let Some(path) = selected_candidate(app).map(|c| c.path.clone()) else {
        return;
    };
    crate::explorer_preview::request_preview(app, &path, effects);
}

pub(crate) fn handle_recents_loaded(
    app: &mut App,
    generation: crate::generation::FileSearchGen,
    result: Result<Vec<Candidate>, CmdError>,
    effects: &mut Effects,
) {
    let current = app.filesearch().map(|s| s.generation);
    if current != Some(generation) {
        return;
    }
    match result {
        Ok(mut recents) => {
            for (index, candidate) in recents.iter_mut().enumerate() {
                candidate.mru_rank = Some(index);
            }
            if let Some(state) = app.filesearch_mut() {
                state.recents = recents;
            }
        }
        Err(e) => {
            crate::messages::error(app, format!("recent files not loaded: {e}"));
        }
    }
    recompute(app, effects);
}

pub(crate) fn reset_and_recompute(app: &mut App, effects: &mut Effects) {
    recompute_core(app, effects, false);
}

pub(crate) fn recompute(app: &mut App, effects: &mut Effects) {
    recompute_core(app, effects, true);
}

fn recompute_core(app: &mut App, effects: &mut Effects, preserve_selection: bool) {
    if app.filesearch().is_none() {
        return;
    }
    let previous_path = selected_candidate(app).map(|c| c.path.clone());
    let restore_path = if preserve_selection {
        previous_path.clone()
    } else {
        None
    };
    let height = keys::page_amount(app).max(1) as usize;

    let Some(state) = app.filesearch_mut() else {
        return;
    };
    if state.query.trim().is_empty() {
        list_all(state);
    } else {
        rank::rank(state);
    }

    let len = state.results.len();
    let cursor = restore_path
        .and_then(|path| find_row_for_path(state, &path))
        .unwrap_or(0);
    state.nav.cursor = cursor;
    state.nav.settle(len, height);

    let selected_now = selected_candidate(app).map(|c| c.path.clone());
    if selected_now != previous_path {
        after_cursor_move(app, effects);
    }
}

fn find_row_for_path(state: &FileSearchState, path: &Path) -> Option<usize> {
    state.results.iter().position(|row| {
        candidate_by(&state.recents, &state.walk, row.candidate_idx).is_some_and(|c| c.path == path)
    })
}

fn list_all(state: &mut FileSearchState) {
    let mut order: Vec<usize> = Vec::with_capacity(state.recents.len() + state.walk.len());
    order.extend(
        state
            .recents
            .iter()
            .enumerate()
            .filter(|(_, c)| c.in_tree)
            .map(|(i, _)| i),
    );
    order.extend(
        state
            .recents
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.in_tree)
            .map(|(i, _)| i),
    );
    let recents_len = state.recents.len();
    order.extend((0..state.walk.len()).map(|i| recents_len + i));
    order.truncate(RESULT_CAP);
    state.results = order
        .into_iter()
        .map(|candidate_idx| ResultRow {
            candidate_idx,
            indices: Vec::new(),
        })
        .collect();
}

pub(crate) fn handle_scanned(
    app: &mut App,
    generation: crate::generation::FileSearchGen,
    result: Result<walk::ScanResult, String>,
    effects: &mut Effects,
) {
    let current = app.filesearch().map(|s| s.generation);
    if current != Some(generation) {
        return;
    }
    match result {
        Ok(scan) => {
            if let Some(state) = app.filesearch_mut() {
                let root = state.root.clone();
                let seen: std::collections::HashSet<PathBuf> =
                    state.recents.iter().map(|c| c.path.clone()).collect();
                state.walk = scan
                    .files
                    .into_iter()
                    .filter(|path| !seen.contains(path))
                    .map(|path| {
                        let display = display_relative(&root, &path);
                        Candidate {
                            path,
                            display,
                            in_tree: true,
                            mru_rank: None,
                        }
                    })
                    .collect();
                state.walk_pending = false;
                state.walk_truncated = scan.truncated;
            }
        }
        Err(e) => {
            if let Some(state) = app.filesearch_mut() {
                state.walk_pending = false;
            }
            crate::messages::warn(app, format!("workspace scan failed: {e}"));
        }
    }
    recompute(app, effects);
}

fn display_relative(root: &Path, path: &Path) -> String {
    if root.as_os_str().is_empty() {
        return path.display().to_string();
    }
    path.strip_prefix(root).map_or_else(
        |_| path.display().to_string(),
        |rel| rel.display().to_string(),
    )
}
