//! `FileSearchState` — the fuzzy file finder overlay's own state, shaped
//! after the in-file search bar's own (`App::search`, `search/mod.rs`):
//! present on `App` only while the finder is open, never a `Pane`
//! (`FocusTarget::FileSearch`, `focus.rs`) and, while open, the underlying
//! chrome `Pane` stays `Explorer`. This module owns open/close/cancel and
//! the recompute chokepoint (render never filters or ranks); keystroke
//! handling lives in the [`keys`] submodule, the query row and result list
//! render through `render::filesearch`.

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

/// The displayed-result cap (plan A5, VS Code's own figure) — the readout's
/// `matched/total` keeps the true count visible even once a query (or, once
/// a later work package's walk lands, an unfiltered listing) produces more
/// candidates than this.
const RESULT_CAP: usize = 512;

/// One path the finder can offer: a document already in the recovery
/// store's MRU list, or a file the ignore-aware workspace walk found.
/// `in_tree`/`mru_rank` drive the ranking a later work package adds;
/// `display` is the string actually matched and shown — root-relative for
/// an in-tree path, absolute otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub display: String,
    pub in_tree: bool,
    pub mru_rank: Option<usize>,
}

/// One ranked row in the finder's result list: which candidate it names,
/// and the matched `nucleo_matcher::Utf32Str` positions `render::
/// filesearch` bolds — empty until a non-empty query runs the candidate
/// through `rank::rank`. `Utf32Str`'s unit is NOT stable across strings:
/// it is raw UTF-8 byte offsets when every grapheme's leading codepoint is
/// ASCII (which includes NFD names like "café.md"), grapheme positions
/// otherwise — the render side re-derives that same branch decision per
/// string rather than assuming one unit.
pub struct ResultRow {
    pub candidate_idx: usize,
    pub indices: Vec<u32>,
}

/// The finder's complete state (`App::filesearch`). `matcher`/`charbuf` are
/// the nucleo matcher's own long-lived scratch space, held for the whole
/// session rather than minted per keystroke.
pub struct FileSearchState {
    pub query: String,
    pub nav: listnav::List,
    pub generation: crate::generation::FileSearchGen,
    pub return_to: crate::returnto::ReturnTo,
    /// The workspace walk's own root, resolved once at [`open`] (A4's own
    /// ladder: `app.root` when resolved, else `explorer::initial_root`)
    /// and reused for both the walk `Cmd` and every candidate's root-
    /// relative `display` string — recomputing it later from live `App`
    /// state would drift out of step with the root the in-flight scan
    /// `Cmd` actually captured, since arrowing through results can itself
    /// move `app.active` (the preview machinery switches documents).
    pub root: PathBuf,
    pub recents: Vec<Candidate>,
    pub walk: Vec<Candidate>,
    pub walk_pending: bool,
    pub walk_truncated: bool,
    pub results: Vec<ResultRow>,
    /// The nucleo matcher's own long-lived scratch space, held for the
    /// whole finder session (`rank`, the only reader) rather than minted
    /// per keystroke. Private: render never touches it (all matching
    /// happens in `update`, never in render).
    matcher: Matcher,
    /// Reusable `Utf32Str::new` scratch buffer, same reasoning as
    /// `matcher` above.
    charbuf: Vec<char>,
}

/// Opens the finder: closes the in-file search bar and the Explorer's own
/// type-to-search (mutual exclusion), remembers the document to restore on
/// cancel, mints a fresh generation, then — LOAD-BEARING ORDER — assigns
/// `app.filesearch` before moving focus: `set_focus_pane` resolves through
/// `layout_mode()`, which only treats the (default-hidden) left column as
/// visible once the state it is about to focus already exists. A no-op if
/// already open.
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

/// A4's own root ladder: `app.root`, when the user has resolved a workspace
/// root, is authoritative. When it's still the startup default (`None`),
/// this falls back to `explorer::initial_root`'s own ladder —
/// which itself prefers the ACTIVE DOCUMENT'S OWN DIRECTORY over `app.root`.
/// So an unresolved `app.root` does not mean the finder walks "the
/// workspace": it walks wherever the currently active document happens to
/// live, only falling back to `app.root`/`.` when that document has no path
/// of its own.
fn resolve_root(app: &App) -> PathBuf {
    let Some(root) = app.root.as_deref() else {
        return crate::explorer::initial_root(app);
    };
    app.vfs.resolve(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Drops the finder's state outright — the plain close every other writer
/// funnels through; [`cancel`] below is the Esc/toggle-close/Trash-refusal
/// path that also restores what was showing before the finder opened.
pub(crate) fn close(app: &mut App) {
    app.close_filesearch();
}

/// Closes the finder and restores `return_to` if it still exists, landing
/// focus back on the Editor either way — the shared tail every path that
/// must undo the finder's own document switch (Esc, a second toggle press,
/// a refused Trash) funnels through.
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

/// The wheel over the finder's own rect moves the SELECTION, through the
/// same nav the Up/Down keys drive — the result list has no scroll of its
/// own to move independently of its cursor.
pub(crate) fn mouse(app: &mut App, input: MouseInput, effects: &mut Effects) {
    match input.kind {
        MouseKind::ScrollUp => keys::nav_move(app, -WHEEL_ROWS, effects),
        MouseKind::ScrollDown => keys::nav_move(app, WHEEL_ROWS, effects),
        _ => {}
    }
}

/// Resolves a `ResultRow::candidate_idx` into the `Candidate` it names:
/// `0..recents.len()` addresses `recents`, the remainder addresses `walk` —
/// the two backing lists `recompute` draws its combined ordering from,
/// never physically merged into one `Vec`. The explicit-slices shape below
/// is the chokepoint both this and `rank` (which only ever has the two
/// slices, not a whole `&FileSearchState`, mid-recompute) route through.
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

/// The candidate the nav cursor currently names, or `None` when the finder
/// is closed or nothing is selected (an empty result list, a cursor past
/// the end). The one place both `keys::open_selected` and
/// [`after_cursor_move`] resolve "what's selected right now" from, so the
/// two can never disagree about it.
pub(crate) fn selected_candidate(app: &App) -> Option<&Candidate> {
    let state = app.filesearch()?;
    let row = state.results.get(state.nav.cursor)?;
    candidate_at(state, row.candidate_idx)
}

/// Requests a live preview of whatever the nav cursor currently selects —
/// the finder's own counterpart of `explorer_keys`'s nav handlers, riding
/// the SAME shared core (`explorer_preview::request_preview`) rather than a
/// parallel reply path. Called after every nav command (`keys::apply`) and
/// from [`recompute`] whenever a keystroke re-ranks the list and the top
/// hit (or the whole list) changes under an unmoved cursor. A no-op with
/// nothing selected — typing into an empty result list requests nothing.
pub(crate) fn after_cursor_move(app: &mut App, effects: &mut Effects) {
    let Some(path) = selected_candidate(app).map(|c| c.path.clone()) else {
        return;
    };
    crate::explorer_preview::request_preview(app, &path, effects);
}

/// Applies a `Msg::RecentsLoaded` file-search reply: dropped outright when the
/// finder has since closed, or when `generation` no longer matches the
/// still-open finder's own `generation` — a close-then-reopen since issued
/// it must never let a late reply land in the session that superseded it,
/// mirroring `search::handle_history_loaded`'s own check. An `Err` reports
/// through the message log and leaves `recents` empty (there's nothing
/// durable to preserve — this is the finder's first load); an `Ok` list
/// adopts `mru_rank = Some(index)` at the position it arrived in, already
/// MRU-ordered by `recent_paths`' own `last_seen_at DESC`.
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

/// Query-edit recompute (`keys::apply`'s `Type`/`Erase` arms, `keys::
/// paste`): the caller itself just changed the FILTER, so a cursor left
/// where it was could now name a completely unrelated row (or point past
/// the end of a shorter list) — always jumps back to the top of the freshly
/// filtered/ranked list. Shares the change-only preview trigger with
/// [`recompute`] below (`recompute_core`'s own doc explains why).
pub(crate) fn reset_and_recompute(app: &mut App, effects: &mut Effects) {
    recompute_core(app, effects, false);
}

/// The recompute-over-cache chokepoint every `recents`/`walk` data reply
/// funnels through (`handle_recents_loaded`, `handle_scanned`) — an empty
/// query lists in-tree recents (MRU order) then out-of-tree recents (MRU
/// order), then `walk` files in whatever order they arrived; a non-empty
/// query fuzzy-ranks through [`rank::rank`] (all scoring and highlight-index
/// computation happens there, never in render). Unlike [`reset_and_
/// recompute`], a data reply did not come from the user changing anything —
/// re-finds the path that was selected before the rebuild and keeps the
/// cursor on it, falling back to the top only once that path is gone from
/// the fresh results. A late reply must never snap the cursor (and the live
/// preview riding it) away from a row the user already arrowed onto.
pub(crate) fn recompute(app: &mut App, effects: &mut Effects) {
    recompute_core(app, effects, true);
}

/// The shared rebuild `reset_and_recompute`/`recompute` both funnel
/// through: rebuilds `results` from the live query, repositions the cursor
/// per `preserve_selection`, then fires [`after_cursor_move`] iff the
/// SELECTED PATH actually changed across the rebuild — comparing paths
/// (not row indices, not query strings) is what lets two edits that
/// happen to share the same top hit skip a redundant preview request,
/// and what lets a data reply that reshuffles indices still recognize the
/// user's own selection survived.
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

/// The row (if any) whose candidate names `path` in the just-rebuilt
/// `state.results` — [`recompute_core`]'s own lookup for relocating a
/// preserved selection after a rebuild that may have reordered everything
/// around it.
fn find_row_for_path(state: &FileSearchState, path: &Path) -> Option<usize> {
    state.results.iter().position(|row| {
        candidate_by(&state.recents, &state.walk, row.candidate_idx).is_some_and(|c| c.path == path)
    })
}

/// The empty-query listing: in-tree recents (MRU order) then out-of-tree
/// recents (MRU order), then `walk` files in scan order — capped at
/// [`RESULT_CAP`]. No highlight indices: nothing was matched against.
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

/// Applies a `Msg::FileSearchScanned` reply — the walk `Cmd` [`open`]
/// pushes, landed. Dropped outright when `generation` no longer matches the
/// still-open finder's own (a close-then-reopen since issued it), mirroring
/// `explorer_dirload::handle_dir_loaded`'s own generation check. A scan
/// failure (the resolved root vanished or wasn't a directory) surfaces once
/// through the message log and leaves `walk` empty rather than leaving
/// `walk_pending` stuck true forever. Every path already present in
/// `recents` (by path equality) is dropped from the fresh walk results
/// instead of duplicated — the recent's own `Candidate`, carrying its
/// `mru_rank`, stays the sole entry for it.
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

/// A candidate's own display string: root-relative when it falls under
/// `root` (every walk result always does — `root` is exactly where its scan
/// started), the full path otherwise. Guards `root` being empty (A4's own
/// legal state) the same way every other in-tree test in this feature does:
/// `Path::starts_with`/`strip_prefix` against an empty root would otherwise
/// classify every path as in-tree.
fn display_relative(root: &Path, path: &Path) -> String {
    if root.as_os_str().is_empty() {
        return path.display().to_string();
    }
    path.strip_prefix(root).map_or_else(
        |_| path.display().to_string(),
        |rel| rel.display().to_string(),
    )
}
