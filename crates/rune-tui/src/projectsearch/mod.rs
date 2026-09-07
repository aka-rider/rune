use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::runtime::{Effects, Msg, TimerKey, TimerMsgKey};

pub(crate) mod index;
pub(crate) mod query;

use index::{ProjectIndexState, ReadOutcome};

const SPINNER_INTERVAL: Duration = Duration::from_millis(100);
const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub(crate) fn spinner_char(frame: u8) -> char {
    let index = usize::from(frame) % SPINNER_FRAMES.len();
    SPINNER_FRAMES.get(index).copied().unwrap_or('⠋')
}

pub(crate) fn ensure_index(app: &mut App, effects: &mut Effects) {
    let root = crate::filesearch::resolve_root(app);
    if app
        .project_index
        .as_ref()
        .is_some_and(|state| state.root == root)
    {
        refresh_index(app, effects);
        return;
    }
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

fn refresh_index(app: &mut App, effects: &mut Effects) {
    let build_generation = app.next_project_index_gen.mint();
    let vfs = Arc::clone(&app.vfs);
    let Some(state) = app.project_index.as_mut() else {
        return;
    };
    state.build_generation = build_generation;
    state.building = true;
    state.pending.clear();
    let root = state.root.clone();
    effects.cmds.push(crate::runtime::project_scan_cmd(
        vfs,
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
    if !crate::find::project::active(app) {
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
            let fingerprints: std::collections::HashMap<&Path, index::Fingerprint> = state
                .entries
                .iter()
                .map(|entry| (entry.path.as_path(), (entry.size, entry.mtime)))
                .collect();
            state.pending = scan
                .files
                .iter()
                .map(|path| (path.clone(), fingerprints.get(path.as_path()).copied()))
                .collect();
            if !scan.truncated {
                let scanned: std::collections::HashSet<&Path> =
                    scan.files.iter().map(PathBuf::as_path).collect();
                state
                    .entries
                    .retain(|entry| scanned.contains(entry.path.as_path()));
                state.corpus_bytes = state.entries.iter().map(|entry| entry_bytes(entry)).sum();
            }
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
                let displaced = remove_entry(state, &entry.path);
                state.corpus_bytes =
                    state.corpus_bytes.saturating_sub(displaced) + entry_bytes(&entry);
                state.entries.push(Arc::new(entry));
            }
            ReadOutcome::Skipped(path) => {
                let displaced = remove_entry(state, &path);
                state.corpus_bytes = state.corpus_bytes.saturating_sub(displaced);
            }
            ReadOutcome::Unchanged(_) => {}
        }
    }
    dispatch_next_batch(app, effects);
}

fn entry_bytes(entry: &index::IndexEntry) -> usize {
    entry.text.len()
}

fn remove_entry(state: &mut ProjectIndexState, path: &Path) -> usize {
    let Some(position) = state.entries.iter().position(|entry| entry.path == path) else {
        return 0;
    };
    let removed = state.entries.remove(position);
    entry_bytes(&removed)
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
        crate::find::project::dispatch_query(app, effects);
        return;
    }
    let take = state.pending.len().min(index::READ_BATCH);
    let batch: Vec<(PathBuf, Option<index::Fingerprint>)> = state.pending.drain(..take).collect();
    effects.cmds.push(crate::runtime::project_read_batch_cmd(
        vfs,
        batch,
        state.root.clone(),
        state.build_generation,
    ));
}
