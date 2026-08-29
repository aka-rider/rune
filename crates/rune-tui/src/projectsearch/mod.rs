use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::pane::Pane;
use crate::pointer::{MouseInput, MouseKind};
use crate::runtime::{Effects, Msg, TimerKey, TimerMsgKey};

pub(crate) mod index;
pub(crate) mod keys;
pub(crate) mod query;

use index::{ProjectIndexState, ReadOutcome};
use query::FileHit;

pub(crate) const MIN_QUERY_CHARS: usize = 2;
const DEBOUNCE_INTERVAL: Duration = Duration::from_millis(120);

pub struct ProjectSearchState {
    pub query: String,
    pub query_generation: crate::generation::ProjectSearchGen,
    pub return_to: crate::returnto::ReturnTo,
    pub results: Vec<FileHit>,
    pub results_truncated: bool,
    pub list: crate::listnav::List,
    pub pending_center: Option<(PathBuf, usize)>,
}

pub(crate) fn open(app: &mut App, effects: &mut Effects) {
    if app.projectsearch().is_some() {
        return;
    }
    let Some(clearance) = app.clear_title_for_overlay(effects) else {
        return;
    };
    app.close_all_overlays(effects);
    crate::explorer_search::clear_search(app);
    let return_to = crate::returnto::ReturnTo::to(app.active);
    let query_generation = app.next_projectsearch_gen.mint();
    let last_query = app
        .project_index
        .as_ref()
        .map(|index| index.last_query.clone())
        .unwrap_or_default();
    app.open_projectsearch(
        ProjectSearchState {
            query: last_query.clone(),
            query_generation,
            return_to,
            results: Vec::new(),
            results_truncated: false,
            list: crate::listnav::List { cursor: 0, top: 0 },
            pending_center: None,
        },
        clearance,
    );
    app.set_focus_pane(Pane::Explorer, effects);
    ensure_index(app, effects);
    if last_query.chars().count() >= MIN_QUERY_CHARS {
        dispatch_query(app, effects);
    }
}

const SPINNER_INTERVAL: Duration = Duration::from_millis(100);
const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub(crate) fn spinner_char(frame: u8) -> char {
    let index = usize::from(frame) % SPINNER_FRAMES.len();
    SPINNER_FRAMES.get(index).copied().unwrap_or('⠋')
}

fn ensure_index(app: &mut App, effects: &mut Effects) {
    if app.project_index.is_some() {
        return;
    }
    let root = crate::filesearch::resolve_root(app);
    let build_generation = app.next_project_index_gen.mint();
    app.project_index = Some(ProjectIndexState {
        root: root.clone(),
        entries: Vec::new(),
        pending: Vec::new(),
        build_generation,
        truncated: false,
        building: true,
        last_query: String::new(),
        corpus_bytes: 0,
        corpus_cap: index::MAX_CORPUS_BYTES,
        spinner_frame: 0,
    });
    effects.cmds.push(crate::runtime::project_scan_cmd(
        Arc::clone(&app.vfs),
        root,
        build_generation,
    ));
    arm_spinner(app, build_generation);
}

fn arm_spinner(app: &App, generation: crate::generation::ProjectIndexGen) {
    app.timers.arm(
        TimerKey::from(TimerMsgKey::ProjectSearchSpinner),
        SPINNER_INTERVAL,
        Msg::Timer {
            key: TimerMsgKey::ProjectSearchSpinner,
            generation: generation.raw(),
        },
    );
}

pub(crate) fn handle_spinner_tick(app: &mut App, generation: u64) {
    if app.projectsearch().is_none() {
        return;
    }
    let Some(state) = app.project_index.as_mut() else {
        return;
    };
    if !state.building || state.build_generation.raw() != generation {
        return;
    }
    state.spinner_frame = state.spinner_frame.wrapping_add(1);
    let build_generation = state.build_generation;
    arm_spinner(app, build_generation);
}

pub(crate) fn handle_index_scanned(
    app: &mut App,
    generation: crate::generation::ProjectIndexGen,
    result: Result<crate::filesearch::walk::ScanResult, String>,
    effects: &mut Effects,
) {
    let Some(state) = app.project_index.as_mut() else {
        return;
    };
    if state.build_generation != generation {
        return;
    }
    match result {
        Ok(scan) => {
            state.truncated = scan.truncated;
            state.pending = scan.files;
            dispatch_next_batch(app, effects);
        }
        Err(e) => {
            state.building = false;
            crate::messages::warn(app, format!("project scan failed: {e}"));
        }
    }
}

pub(crate) fn handle_index_batch(
    app: &mut App,
    generation: crate::generation::ProjectIndexGen,
    outcomes: Vec<ReadOutcome>,
    effects: &mut Effects,
) {
    let Some(state) = app.project_index.as_mut() else {
        return;
    };
    if state.build_generation != generation {
        return;
    }
    for outcome in outcomes {
        match outcome {
            ReadOutcome::Indexed(entry) => {
                state.corpus_bytes += entry.text.len() + entry.folded.len();
                state.entries.push(Arc::new(entry));
            }
            ReadOutcome::Unchanged(_) | ReadOutcome::Skipped(_) => {}
        }
    }
    dispatch_next_batch(app, effects);
}

fn dispatch_next_batch(app: &mut App, effects: &mut Effects) {
    let vfs = Arc::clone(&app.vfs);
    let Some(state) = app.project_index.as_mut() else {
        return;
    };
    if state.corpus_bytes > state.corpus_cap {
        state.truncated = true;
        state.pending.clear();
    }
    if state.pending.is_empty() {
        state.building = false;
        dispatch_query(app, effects);
        return;
    }
    let take = state.pending.len().min(index::READ_BATCH);
    let batch: Vec<(PathBuf, Option<index::Fingerprint>)> = state
        .pending
        .drain(..take)
        .map(|path| (path, None))
        .collect();
    effects.cmds.push(crate::runtime::project_read_batch_cmd(
        vfs,
        batch,
        state.root.clone(),
        state.build_generation,
    ));
}

pub(crate) fn restart_debounce(app: &App) {
    if app.projectsearch().is_none() {
        return;
    }
    app.timers.arm(
        TimerKey::from(TimerMsgKey::ProjectSearchDebounce),
        DEBOUNCE_INTERVAL,
        Msg::Timer {
            key: TimerMsgKey::ProjectSearchDebounce,
            generation: 0,
        },
    );
}

pub(crate) fn handle_debounce(app: &mut App, effects: &mut Effects) {
    if app.projectsearch().is_none() {
        return;
    }
    dispatch_query(app, effects);
}

fn dispatch_query(app: &mut App, effects: &mut Effects) {
    let Some(query) = app.projectsearch().map(|s| s.query.clone()) else {
        return;
    };
    if query.chars().count() < MIN_QUERY_CHARS {
        if let Some(state) = app.projectsearch_mut() {
            state.results.clear();
            state.results_truncated = false;
            state.list = crate::listnav::List { cursor: 0, top: 0 };
        }
        return;
    }
    let Some((entries, root)) = app
        .project_index
        .as_ref()
        .map(|index| (index.entries.clone(), index.root.clone()))
    else {
        return;
    };
    let overrides = gather_overrides(app, &root);
    let generation = app.next_projectsearch_gen.mint();
    let Some(state) = app.projectsearch_mut() else {
        return;
    };
    state.query_generation = generation;
    effects.cmds.push(crate::runtime::project_query_cmd(
        entries, overrides, query, generation,
    ));
}

fn gather_overrides(app: &App, root: &Path) -> Vec<(PathBuf, String)> {
    app.documents
        .values()
        .filter_map(|doc| {
            let path = doc.file_path.as_deref()?;
            let resolved = app.vfs.resolve(path).ok()?;
            if !resolved.starts_with(root) {
                return None;
            }
            let content = doc.buffer.content();
            if content.len() as u64 > index::MAX_INDEX_FILE_BYTES {
                return None;
            }
            Some((resolved, content.to_string()))
        })
        .collect()
}

pub(crate) fn handle_queried(
    app: &mut App,
    generation: crate::generation::ProjectSearchGen,
    results: Vec<FileHit>,
    truncated: bool,
) {
    let Some(state) = app.projectsearch_mut() else {
        return;
    };
    if state.query_generation != generation {
        return;
    }
    state.results = results;
    state.results_truncated = truncated;
    state.list = crate::listnav::List { cursor: 0, top: 0 };
}

pub(crate) fn mouse(app: &mut App, input: MouseInput, effects: &mut Effects) {
    match input.kind {
        MouseKind::ScrollUp => keys::nav_move(app, -crate::commands::mouse::WHEEL_ROWS, effects),
        MouseKind::ScrollDown => keys::nav_move(app, crate::commands::mouse::WHEEL_ROWS, effects),
        _ => {}
    }
}

pub(crate) fn click_row(app: &mut App, visible_row: usize, effects: &mut Effects) {
    let Some(state) = app.projectsearch() else {
        return;
    };
    let height = crate::filesearch::keys::page_amount(app).max(1) as usize;
    let window = state.list.window(state.results.len(), height);
    let Some(absolute) = window.start.checked_add(visible_row) else {
        return;
    };
    if absolute >= window.end {
        return;
    }
    if let Some(state) = app.projectsearch_mut() {
        state.list.cursor = absolute;
    }
    keys::open_selected(app, effects);
}

pub(crate) fn after_selection_change(app: &mut App, effects: &mut Effects) {
    let Some((path, first_match)) = app.projectsearch().and_then(|state| {
        state
            .results
            .get(state.list.cursor)
            .map(|hit| (hit.path.clone(), hit.first_match))
    }) else {
        return;
    };
    crate::explorer_preview::request_preview(app, &path, effects);
    if let Some(state) = app.projectsearch_mut() {
        state.pending_center = Some((path, first_match));
    }
    apply_pending_center(app);
}

pub(crate) fn apply_pending_center(app: &mut App) {
    let Some((path, offset)) = app
        .projectsearch()
        .and_then(|state| state.pending_center.clone())
    else {
        return;
    };
    let Some(id) = crate::workspace::existing_document_for(app, &path) else {
        return;
    };
    if let Some(doc) = app.doc_mut(id) {
        crate::commands::nav_scroll::centre_on_byte_offset(doc, offset);
    }
    if let Some(state) = app.projectsearch_mut() {
        state.pending_center = None;
    }
}

pub(crate) fn close(app: &mut App) {
    let query = app.projectsearch().map(|state| state.query.clone());
    app.close_projectsearch();
    if let (Some(query), Some(index)) = (query, app.project_index.as_mut()) {
        index.last_query = query;
    }
}

pub(crate) fn cancel(app: &mut App, effects: &mut Effects) {
    let Some(return_to) = app.projectsearch().map(|s| s.return_to) else {
        return;
    };
    close(app);
    if let Some(target) = return_to.live(app) {
        crate::workspace::switch_to(app, target);
    }
    app.set_focus_pane(Pane::Editor, effects);
}

pub(crate) fn toggle(app: &mut App, effects: &mut Effects) {
    if app.projectsearch().is_some() {
        cancel(app, effects);
    } else {
        open(app, effects);
    }
}

#[cfg(test)]
mod preview_tests;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod tests;
