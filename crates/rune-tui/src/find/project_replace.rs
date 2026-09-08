use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rune_core::buffer::Edit;
use rune_core::cursor::CursorId;
use rune_core::undo::EditKind;

use crate::app::App;
use crate::commands::edit_core::apply_edit_batch_with_cursors;
use crate::document::{DocumentId, Replica};
use crate::find::matcher::Matcher;
use crate::find::project::ProjectResults;
use crate::find::replace::collapsed_after_the_last_edit;
use crate::find::{history, project};
use crate::messages;
use crate::opentabs::limit::{MAX_TABS, ensure_room};
use crate::runtime::Effects;
use crate::workspace;

pub(crate) struct Walk {
    pub pattern: Matcher,
    pub replacement: String,
    pub queued: Vec<DocumentId>,
    pub done: usize,
    pub unmatched: usize,
    pub skipped: usize,
    pub diverged: Vec<String>,
    pub unrecoverable: Vec<String>,
    pub total: usize,
    pub stopped_at_limit: bool,
}

impl Walk {
    fn new(pattern: Matcher, replacement: String, total: usize) -> Walk {
        Walk {
            pattern,
            replacement,
            queued: Vec::new(),
            done: 0,
            unmatched: 0,
            skipped: 0,
            diverged: Vec::new(),
            unrecoverable: Vec::new(),
            total,
            stopped_at_limit: false,
        }
    }

    fn record(&mut self, outcome: FileOutcome) {
        match outcome {
            FileOutcome::Applied => self.done += 1,
            FileOutcome::NoMatches => self.unmatched += 1,
            FileOutcome::ReadOnly
            | FileOutcome::ReadFailed
            | FileOutcome::Diverged
            | FileOutcome::Unrecoverable => self.skipped += 1,
            FileOutcome::Queued => {}
            FileOutcome::TabLimit => self.stopped_at_limit = true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileOutcome {
    Applied,
    NoMatches,
    ReadOnly,
    Diverged,
    Unrecoverable,
    Queued,
    ReadFailed,
    TabLimit,
}

fn walk(app: &App) -> Option<&Walk> {
    app.find()?.project.as_ref()?.walk.as_deref()
}

fn walk_mut(app: &mut App) -> Option<&mut Walk> {
    app.find_mut()?.project.as_mut()?.walk.as_deref_mut()
}

pub(crate) fn replace_all_project(app: &mut App, effects: &mut Effects) {
    let Some(paths) = app
        .find()
        .and_then(|state| state.project.as_ref())
        .map(|project| project.results.iter().map(|hit| hit.path.clone()).collect())
    else {
        return;
    };
    start_walk(app, paths, effects);
}

pub(crate) fn replace_selected(app: &mut App, effects: &mut Effects) {
    let selected = app
        .find()
        .and_then(|state| state.project.as_ref())
        .and_then(ProjectResults::selected)
        .map(|hit| hit.path.clone());
    start_walk(app, selected.into_iter().collect(), effects);
}

fn start_walk(app: &mut App, paths: Vec<PathBuf>, effects: &mut Effects) {
    if walk(app).is_some() {
        messages::info(app, "replace in progress");
        return;
    }
    let Some(replacement) = app
        .find()
        .and_then(|state| state.replace.as_ref())
        .map(|field| field.editor.text().to_string())
    else {
        return;
    };
    let pattern = app
        .find()
        .and_then(|state| state.pattern.as_ref().ok())
        .filter(|_| !paths.is_empty())
        .cloned();
    let Some(pattern) = pattern else {
        messages::info(app, "no match to replace");
        return;
    };
    if let Some(project) = app.find_mut().and_then(|state| state.project.as_mut()) {
        project.walk = Some(Box::new(Walk::new(pattern, replacement, paths.len())));
    }
    for path in paths {
        let outcome = replace_in_file(app, &path, effects);
        let Some(walk) = walk_mut(app) else {
            return;
        };
        walk.record(outcome);
        if outcome == FileOutcome::TabLimit {
            break;
        }
    }
    settle(app, effects);
    if let Some(waiting) = walk(app).map(|walk| walk.queued.len()) {
        messages::info(app, format!("replacing in {} as they load", files(waiting)));
    }
}

pub(crate) fn replace_in_file(app: &mut App, path: &Path, effects: &mut Effects) -> FileOutcome {
    let id = match workspace::existing_document_for_spelling(app, path) {
        Some(id) => id,
        None => {
            let holding_queued = walk(app).is_some_and(|walk| !walk.queued.is_empty());
            if holding_queued && app.documents.order().len() >= MAX_TABS {
                return FileOutcome::TabLimit;
            }
            if !ensure_room(app, effects) {
                return FileOutcome::TabLimit;
            }
            match workspace::open_path_checked(app, path, effects) {
                Some(id) => id,
                None => return FileOutcome::ReadFailed,
            }
        }
    };
    if app
        .doc(id)
        .is_some_and(|doc| matches!(doc.replica, Replica::Binding { .. }))
    {
        if let Some(walk) = walk_mut(app) {
            walk.queued.push(id);
        }
        return FileOutcome::Queued;
    }
    apply_to_document(app, id)
}

pub(crate) fn apply_to_document(app: &mut App, id: DocumentId) -> FileOutcome {
    let Some((pattern, replacement)) =
        walk(app).map(|walk| (walk.pattern.clone(), walk.replacement.clone()))
    else {
        return FileOutcome::ReadFailed;
    };
    let Some(doc) = app.doc(id) else {
        return FileOutcome::ReadFailed;
    };
    if doc.last_sync == Some(rune_db::SyncKind::Diverged) {
        keep_unchanged(app, id, |walk| &mut walk.diverged);
        return FileOutcome::Diverged;
    }
    let cursors_before = doc.cursors.clone();
    let cursor = cursors_before.primary().id;
    let edits: Vec<(Edit, CursorId)> = pattern
        .replacements(doc.buffer.content(), &replacement)
        .into_iter()
        .map(|(range, insert)| {
            (
                Edit {
                    start: range.start,
                    end: range.end,
                    insert,
                },
                cursor,
            )
        })
        .collect();
    if edits.is_empty() {
        return FileOutcome::NoMatches;
    }
    let applied = apply_edit_batch_with_cursors(
        app,
        id,
        edits,
        &cursors_before,
        EditKind::Other,
        collapsed_after_the_last_edit,
    );
    if !applied {
        return FileOutcome::ReadOnly;
    }
    crate::save::schedule_snapshot_debounce(app, id);
    FileOutcome::Applied
}

pub(crate) fn settle(app: &mut App, effects: &mut Effects) {
    let Some(queued) = walk_mut(app).map(|walk| std::mem::take(&mut walk.queued)) else {
        return;
    };
    let mut waiting = Vec::new();
    for id in queued {
        match settled_outcome(app, id) {
            None => waiting.push(id),
            Some(outcome) => {
                if let Some(walk) = walk_mut(app) {
                    walk.record(outcome);
                }
            }
        }
    }
    let Some(walk) = walk_mut(app) else {
        return;
    };
    walk.queued = waiting;
    if walk.queued.is_empty() {
        finish(app, effects);
    }
}

fn settled_outcome(app: &mut App, id: DocumentId) -> Option<FileOutcome> {
    let Some(doc) = app.doc(id) else {
        return Some(FileOutcome::ReadFailed);
    };
    if matches!(doc.replica, Replica::Binding { .. }) {
        return None;
    }
    if doc.replica.is_bound() {
        return Some(apply_to_document(app, id));
    }
    keep_unchanged(app, id, |walk| &mut walk.unrecoverable);
    Some(FileOutcome::Unrecoverable)
}

fn keep_unchanged(app: &mut App, id: DocumentId, kept: fn(&mut Walk) -> &mut Vec<String>) {
    let Some(name) = app.doc(id).map(|doc| doc.file_name().to_string()) else {
        return;
    };
    if let Some(walk) = walk_mut(app) {
        kept(walk).push(name);
    }
}

fn finish(app: &mut App, effects: &mut Effects) {
    let Some((walk, query, options, origin)) = app.find_mut().and_then(|state| {
        let walk = state.project.as_mut()?.walk.take()?;
        Some((
            walk,
            state.find.editor.text().to_string(),
            state.options,
            state.origin.doc,
        ))
    }) else {
        return;
    };
    let origin_evicted = !app.documents.contains_key(&origin);
    let text = summary(&walk, origin_evicted);
    if walk.stopped_at_limit {
        messages::warn(app, text);
    } else {
        messages::info(app, text);
    }
    if walk.done > 0 {
        history::persist_query(app, &query, options);
        history::persist_replacement(app, &walk.replacement);
    }
    project::dispatch_query(app, effects);
}

fn summary(walk: &Walk, origin_evicted: bool) -> String {
    let mut text = if walk.stopped_at_limit {
        format!(
            "replaced in {} of {} files; free a tab, then press All again",
            walk.done, walk.total
        )
    } else {
        format!("replaced in {}", files(walk.done))
    };
    if walk.skipped > 0 {
        let _ = write!(text, ", {} skipped", walk.skipped);
    }
    if walk.unmatched > 0 {
        let _ = write!(text, ", {} already had no matches", walk.unmatched);
    }
    if !walk.diverged.is_empty() {
        let _ = write!(
            text,
            "; {} kept unchanged: disk changed under recovered edits, merge first",
            walk.diverged.join(", ")
        );
    }
    if !walk.unrecoverable.is_empty() {
        let _ = write!(
            text,
            "; {} kept unchanged: crash recovery unavailable",
            walk.unrecoverable.join(", ")
        );
    }
    if origin_evicted {
        text.push_str("; the tab you started from was closed to make room");
    }
    text
}

pub(crate) fn abandon(app: &mut App, walk: Option<Box<Walk>>) {
    let Some(walk) = walk else {
        return;
    };
    let waiting = walk.queued.len();
    if waiting == 0 {
        return;
    }
    let verb = if waiting == 1 { "was" } else { "were" };
    messages::warn(
        app,
        format!("{} {verb} opened but not replaced", files(waiting)),
    );
}

fn files(n: usize) -> String {
    if n == 1 {
        "1 file".to_string()
    } else {
        format!("{n} files")
    }
}
