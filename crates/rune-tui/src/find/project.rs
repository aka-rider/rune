use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ratatui::layout::Rect;

use crate::app::App;
use crate::commands::mouse::{WHEEL_ROWS, select_range};
use crate::document::DocumentId;
use crate::find::project_replace::{self, Walk};
use crate::find::{Control, Filter, FilterKind, FindState, Origin, history};
use crate::listnav::List;
use crate::messages;
use crate::pointer::{MouseButton, MouseInput, MouseKind};
use crate::projectsearch::index;
use crate::projectsearch::query::FileHit;
use crate::runtime::{Effects, Msg, TimerKey, TimerMsgKey};
use crate::viewport::ScrollMode;

pub(crate) const MIN_QUERY_CHARS: usize = 2;
const DEBOUNCE_INTERVAL: Duration = Duration::from_millis(120);

pub(crate) struct ProjectResults {
    pub results: Vec<FileHit>,
    pub truncated: bool,
    pub list: List,
    pub query_generation: crate::generation::ProjectSearchGen,
    pub pending_center: Option<(PathBuf, usize)>,
    pub walk: Option<Box<Walk>>,
}

impl ProjectResults {
    fn new(query_generation: crate::generation::ProjectSearchGen) -> ProjectResults {
        ProjectResults {
            results: Vec::new(),
            truncated: false,
            list: List { cursor: 0, top: 0 },
            query_generation,
            pending_center: None,
            walk: None,
        }
    }

    pub(crate) fn selected(&self) -> Option<&FileHit> {
        self.results.get(self.list.cursor)
    }

    pub(crate) fn match_total(&self) -> usize {
        self.results.iter().map(|hit| hit.count).sum()
    }

    fn clear(&mut self) {
        self.results.clear();
        self.truncated = false;
        self.list = List { cursor: 0, top: 0 };
    }
}

pub(crate) fn active(app: &App) -> bool {
    app.find().is_some_and(|state| state.project().is_some())
}

pub(crate) fn listed(app: &App) -> bool {
    app.find().is_some_and(FindState::results_listed)
}

fn results(app: &App) -> Option<&ProjectResults> {
    app.find().and_then(FindState::project)
}

fn results_mut(app: &mut App) -> Option<&mut ProjectResults> {
    app.find_mut().and_then(FindState::project_mut)
}

pub(crate) fn apply_filter(app: &mut App, kind: FilterKind, effects: &mut Effects) {
    let Some(current) = app.find().map(FindState::filter_kind) else {
        return;
    };
    if current == kind {
        return;
    }
    match kind {
        FilterKind::Workspace => {
            let generation = app.next_projectsearch_gen.mint();
            if let Some(state) = app.find_mut() {
                state.filter = Filter::Workspace {
                    results: ProjectResults::new(generation),
                    globs: Box::default(),
                };
            }
            crate::projectsearch::ensure_index(app, effects);
        }
        FilterKind::ThisFile => {
            let Some(state) = app.find_mut() else {
                return;
            };
            let Filter::Workspace { results, .. } =
                std::mem::replace(&mut state.filter, Filter::ThisFile)
            else {
                return;
            };
            state.settle_focus();
            crate::explorer_preview::discard(app);
            project_replace::abandon(app, results.walk);
        }
    }
}

pub(crate) fn set_filter(app: &mut App, kind: FilterKind, effects: &mut Effects) {
    apply_filter(app, kind, effects);
    crate::find::requery(app);
}

pub(crate) fn toggle_this_file(app: &mut App, effects: &mut Effects) {
    let Some(kind) = app.find().map(FindState::filter_kind) else {
        return;
    };
    let next = match kind {
        FilterKind::ThisFile => FilterKind::Workspace,
        FilterKind::Workspace => FilterKind::ThisFile,
    };
    set_filter(app, next, effects);
}

pub(crate) fn restart_debounce(app: &App) {
    if !active(app) {
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
    if active(app) {
        dispatch_query(app, effects);
    }
}

pub(crate) fn dispatch_query(app: &mut App, effects: &mut Effects) {
    let Some(state) = app.find() else {
        return;
    };
    let Some(globs) = state.globs() else {
        return;
    };
    let matcher = match &state.pattern {
        Ok(matcher) if state.find.editor.text().chars().count() >= MIN_QUERY_CHARS => {
            matcher.clone()
        }
        _ => {
            abandon_query(app);
            return;
        }
    };
    let Ok(path_filter) = globs.path_filter() else {
        abandon_query(app);
        return;
    };
    let Some((entries, root)) = app
        .project_index
        .as_ref()
        .map(|index| (index.entries.clone(), index.root.clone()))
    else {
        return;
    };
    let entries: Vec<_> = entries
        .into_iter()
        .filter(|entry| path_filter.admits(&root, &entry.path))
        .collect();
    let overrides: Vec<_> = gather_overrides(app, &root)
        .into_iter()
        .filter(|(path, _)| path_filter.admits(&root, path))
        .collect();
    let generation = app.next_projectsearch_gen.mint();
    let Some(project) = results_mut(app) else {
        return;
    };
    project.query_generation = generation;
    effects.cmds.push(crate::runtime::project_query_cmd(
        entries, overrides, matcher, generation,
    ));
}

fn abandon_query(app: &mut App) {
    let generation = app.next_projectsearch_gen.mint();
    if let Some(state) = app.find_mut() {
        if let Some(project) = state.project_mut() {
            project.query_generation = generation;
            project.clear();
        }
        state.settle_focus();
    }
}

fn gather_overrides(app: &App, root: &Path) -> Vec<(PathBuf, String)> {
    app.documents
        .values()
        .filter_map(|doc| {
            let resolved = doc.resolved_path()?.clone().into_path_buf();
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
    effects: &mut Effects,
) {
    let Some(project) = results_mut(app) else {
        return;
    };
    if project.query_generation != generation {
        return;
    }
    project.results = results;
    project.truncated = truncated;
    project.list = List { cursor: 0, top: 0 };
    if let Some(state) = app.find_mut() {
        state.settle_focus();
    }
    focus_selected_hit(app, effects);
}

fn selected_hit(app: &App) -> Option<(PathBuf, Range<usize>)> {
    let hit = results(app)?.selected()?;
    let range = hit
        .ranges
        .first()
        .cloned()
        .unwrap_or(hit.first_match..hit.first_match);
    Some((hit.path.clone(), range))
}

fn focus_selected_hit(app: &mut App, effects: &mut Effects) {
    let Some((path, range)) = selected_hit(app) else {
        return;
    };
    if crate::workspace::existing_document_for_spelling(app, &path) == Some(app.active) {
        land_at(app, app.active, range);
        return;
    }
    crate::find::navigating_as_panel(app, |app| {
        crate::explorer_preview::request_preview(app, &path, effects);
    });
    if let Some(project) = results_mut(app) {
        project.pending_center = Some((path, range.start));
    }
    apply_pending_center(app);
}

pub(crate) fn apply_pending_center(app: &mut App) {
    let Some((path, offset)) = results(app).and_then(|project| project.pending_center.clone())
    else {
        return;
    };
    let Some(target) = crate::workspace::shown_document_for(app, &path) else {
        return;
    };
    if let Some(doc) = app.live_doc_mut(target) {
        crate::commands::nav_scroll::centre_on_byte_offset(doc, offset);
    }
    if let Some(project) = results_mut(app) {
        project.pending_center = None;
    }
}

pub(crate) fn list_height(app: &App) -> usize {
    let area = app.frame_area();
    usize::from(crate::layout::geometry(area, app).explorer_inner.height).max(1)
}

pub(crate) fn nav_move(app: &mut App, delta: isize, effects: &mut Effects) {
    let height = list_height(app);
    let Some(project) = results_mut(app) else {
        return;
    };
    let len = project.results.len();
    project.list.move_and_follow(delta, len, height);
    focus_selected_hit(app, effects);
}

pub(crate) fn nav_edge(app: &mut App, top: bool, effects: &mut Effects) {
    let height = list_height(app);
    let Some(project) = results_mut(app) else {
        return;
    };
    let len = project.results.len();
    project.list.jump_to_edge(len, top);
    project.list.settle(len, height);
    focus_selected_hit(app, effects);
}

pub(crate) fn step_hit(app: &mut App, forward: bool, effects: &mut Effects) {
    let height = list_height(app);
    let Some(project) = results_mut(app) else {
        return;
    };
    let len = project.results.len();
    if len == 0 {
        report_no_results(app);
        return;
    }
    let cursor = project.list.cursor;
    project.list.cursor = if forward {
        (cursor + 1) % len
    } else {
        (cursor + len - 1) % len
    };
    project.list.settle(len, height);
    open_hit(app, effects);
}

fn report_no_results(app: &mut App) {
    let query = app
        .find()
        .map(|state| state.find.editor.text().to_string())
        .unwrap_or_default();
    if query.chars().count() < MIN_QUERY_CHARS {
        messages::info(
            app,
            format!("type at least {MIN_QUERY_CHARS} characters to search the project"),
        );
    } else {
        messages::info(app, format!("no matches for \"{query}\" in the project"));
    }
}

pub(crate) fn open_hit(app: &mut App, effects: &mut Effects) {
    let Some((path, range)) = selected_hit(app) else {
        report_no_results(app);
        return;
    };
    let Some((query, options)) = app
        .find()
        .map(|state| (state.find.editor.text().to_string(), state.options))
    else {
        return;
    };
    let opened = crate::find::navigating_as_panel(app, |app| {
        if crate::explorer_preview::shown_path(app) == Some(path.as_path()) {
            match crate::explorer_preview::promote(app, effects) {
                crate::explorer_preview::Promotion::Promoted(id) => Some(id),
                crate::explorer_preview::Promotion::NothingToPromote
                | crate::explorer_preview::Promotion::Refused => None,
            }
        } else {
            let departed = crate::navhistory::departure_origin(app);
            let id = crate::workspace::open_path_checked(app, &path, effects)?;
            crate::navhistory::record_departure_if_moved(app, departed);
            Some(id)
        }
    });
    let Some(id) = opened else {
        return;
    };
    land_at(app, id, range);
    let origin = Origin::capture(app);
    if let Some(state) = app.find_mut() {
        state.origin = origin;
    }
    history::persist_query(app, &query, options);
}

fn land_at(app: &mut App, id: DocumentId, range: Range<usize>) {
    let Some(doc) = app.doc_mut(id) else {
        return;
    };
    let len = doc.buffer.content().len();
    let start = range.start.min(len);
    let end = range.end.min(len);
    select_range(doc, start, end);
    doc.viewport.mode = ScrollMode::EnsureVisible;
    crate::find::sync(app);
    if let Some(state) = app.find_mut() {
        state.current = state.matches.iter().position(|m| m.start == start);
    }
}

pub(crate) fn mouse(app: &mut App, rect: Rect, input: MouseInput, effects: &mut Effects) {
    match input.kind {
        MouseKind::ScrollUp => nav_move(app, -WHEEL_ROWS, effects),
        MouseKind::ScrollDown => nav_move(app, WHEEL_ROWS, effects),
        MouseKind::Down(MouseButton::Left) => {
            let visible_row = usize::from(input.row.saturating_sub(rect.y));
            click_row(app, visible_row, effects);
        }
        MouseKind::Down(MouseButton::Right | MouseButton::Middle)
        | MouseKind::Up(_)
        | MouseKind::Drag(_) => {}
    }
}

fn click_row(app: &mut App, visible_row: usize, effects: &mut Effects) {
    let height = list_height(app);
    let Some(project) = results_mut(app) else {
        return;
    };
    let window = project.list.window(project.results.len(), height);
    let Some(absolute) = window.start.checked_add(visible_row) else {
        return;
    };
    if absolute >= window.end {
        return;
    }
    project.list.cursor = absolute;
    if let Some(state) = app.find_mut() {
        state.focused = true;
        state.focus = Control::Results;
    }
    open_hit(app, effects);
}

pub(crate) fn readout_text(app: &App, state: &FindState) -> Option<String> {
    let project = state.project()?;
    let index = app.project_index.as_ref()?;
    if index.building {
        return Some(crate::projectsearch::spinner_char(index.spinner_frame).to_string());
    }
    if state.find.editor.text().chars().count() < MIN_QUERY_CHARS {
        return Some(format!("{MIN_QUERY_CHARS}+ chars"));
    }
    let files = project.results.len();
    if files == 0 {
        return Some("no matches".to_string());
    }
    let approx = if index.truncated || project.truncated {
        "\u{2248}"
    } else {
        ""
    };
    let noun = if files == 1 { "file" } else { "files" };
    Some(format!(
        "{approx}{files} {noun} \u{b7} {}",
        project.match_total()
    ))
}
