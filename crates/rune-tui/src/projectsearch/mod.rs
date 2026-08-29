use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::pane::Pane;
use crate::runtime::{Effects, Msg, TimerKey, TimerMsgKey};

pub(crate) mod index;
pub(crate) mod keys;

use index::{ProjectIndexState, ReadOutcome};

pub struct ProjectSearchState {
    pub query: String,
    pub query_generation: crate::generation::ProjectSearchGen,
    pub return_to: crate::returnto::ReturnTo,
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
    app.open_projectsearch(
        ProjectSearchState {
            query: String::new(),
            query_generation,
            return_to,
        },
        clearance,
    );
    app.set_focus_pane(Pane::Explorer, effects);
    ensure_index(app, effects);
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

pub(crate) fn close(app: &mut App) {
    app.close_projectsearch();
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
mod tests;
