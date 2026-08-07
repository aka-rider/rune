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
use crate::document::DocumentId;
use crate::listnav;
use crate::pane::Pane;
use crate::runtime::Effects;
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
/// its match score, and the matched-grapheme byte indices a later work
/// package fills in — `score`/`indices` sit at their zero value until
/// fuzzy ranking lands.
pub struct ResultRow {
    pub candidate_idx: usize,
    pub score: u32,
    pub indices: Vec<u32>,
}

/// The finder's complete state (`App::filesearch`). `matcher`/`charbuf` are
/// the nucleo matcher's own long-lived scratch space, held for the whole
/// session rather than minted per keystroke.
pub struct FileSearchState {
    pub query: String,
    pub nav: listnav::List,
    pub generation: u64,
    pub return_to: DocumentId,
    /// The workspace walk's own root, resolved once at [`open`] (A4's own
    /// ladder: `app.root` when non-empty, else `explorer::initial_root`)
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
    if app.filesearch.is_some() {
        return;
    }
    crate::search::close(app);
    crate::explorer_search::clear_search(app);
    let return_to = app.active;
    app.next_filesearch_gen = app.next_filesearch_gen.wrapping_add(1);
    let generation = app.next_filesearch_gen;
    let root = resolve_root(app);
    let walk_pending = !root.as_os_str().is_empty();
    app.filesearch = Some(FileSearchState {
        query: String::new(),
        nav: listnav::List { cursor: 0, top: 0 },
        generation,
        return_to,
        root: root.clone(),
        recents: Vec::new(),
        walk: Vec::new(),
        walk_pending,
        walk_truncated: false,
        results: Vec::new(),
        matcher: Matcher::new(Config::DEFAULT.match_paths()),
        charbuf: Vec::new(),
    });
    // A4: skipped outright when even `explorer::initial_root`'s own "."
    // fallback resolved to nothing — recents-only is still a useful finder,
    // there just isn't a workspace tree to walk.
    if walk_pending {
        effects.cmds.push(crate::runtime::filesearch_scan_cmd(
            Arc::clone(&app.vfs),
            root.clone(),
            generation,
        ));
    }
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

/// A4's own root ladder: `app.root` takes priority over `explorer::
/// initial_root`'s active-document-directory preference, since the finder
/// searches the WORKSPACE, not wherever the current document happens to
/// live — falling back to `initial_root` only when `app.root` is itself
/// unresolved (still the startup default, an empty `PathBuf`).
fn resolve_root(app: &App) -> PathBuf {
    if app.root.as_os_str().is_empty() {
        return crate::explorer::initial_root(app);
    }
    app.vfs
        .resolve(&app.root)
        .unwrap_or_else(|_| app.root.clone())
}

/// Drops the finder's state outright — the plain close every other writer
/// funnels through; [`cancel`] below is the Esc/toggle-close/Trash-refusal
/// path that also restores what was showing before the finder opened.
pub(crate) fn close(app: &mut App) {
    app.filesearch = None;
}

/// Closes the finder and restores `return_to` if it still exists, landing
/// focus back on the Editor either way — the shared tail every path that
/// must undo the finder's own document switch (Esc, a second toggle press,
/// a refused Trash) funnels through.
pub(crate) fn cancel(app: &mut App, effects: &mut Effects) {
    let Some(return_to) = app.filesearch.as_ref().map(|s| s.return_to) else {
        return;
    };
    close(app);
    if app.doc(return_to).is_some() {
        workspace::switch_to(app, return_to);
    }
    app.set_focus_pane(Pane::Editor, effects);
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
    match idx.checked_sub(recents.len()) {
        None => recents.get(idx),
        Some(walk_idx) => walk.get(walk_idx),
    }
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
    let state = app.filesearch.as_ref()?;
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

/// Applies a `Msg::FileSearchRecentsLoaded` reply: dropped outright when the
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
    generation: u64,
    result: Result<Vec<Candidate>, String>,
    effects: &mut Effects,
) {
    let current = app.filesearch.as_ref().map(|s| s.generation);
    if current != Some(generation) {
        return;
    }
    match result {
        Ok(mut recents) => {
            for (index, candidate) in recents.iter_mut().enumerate() {
                candidate.mru_rank = Some(index);
            }
            if let Some(state) = app.filesearch.as_mut() {
                state.recents = recents;
            }
        }
        Err(e) => {
            crate::messages::error(app, format!("recent files not loaded: {e}"));
        }
    }
    recompute(app, effects);
}

/// The recompute-over-cache chokepoint every query edit (and every
/// `recents`/`walk` load) funnels through — an empty query lists in-tree
/// recents (MRU order) then out-of-tree recents (MRU order), then `walk`
/// files in whatever order they arrived; a non-empty query fuzzy-ranks
/// through [`rank::rank`] (all scoring and highlight-index computation
/// happens there, never in render). Either way the cursor resets to the
/// top of the freshly computed list — a stale cursor position could point
/// at the wrong row, or past the end of a now-shorter one, the same reason
/// `search::recompute` always clears its own `current` — and
/// [`after_cursor_move`] then requests a preview of whatever landed there,
/// which is why every caller already threads `effects` through.
pub(crate) fn recompute(app: &mut App, effects: &mut Effects) {
    let Some(state) = app.filesearch.as_mut() else {
        return;
    };
    if state.query.trim().is_empty() {
        list_all(state);
    } else {
        rank::rank(state);
    }
    if let Some(state) = app.filesearch.as_mut() {
        state.nav.cursor = 0;
        state.nav.top = 0;
    }
    after_cursor_move(app, effects);
}

/// The empty-query listing: in-tree recents (MRU order) then out-of-tree
/// recents (MRU order), then `walk` files in scan order — capped at
/// [`RESULT_CAP`]. No score, no highlight indices: nothing was matched
/// against.
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
            score: 0,
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
    generation: u64,
    result: Result<walk::ScanResult, String>,
    effects: &mut Effects,
) {
    let current = app.filesearch.as_ref().map(|s| s.generation);
    if current != Some(generation) {
        return;
    }
    match result {
        Ok(scan) => {
            if let Some(state) = app.filesearch.as_mut() {
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
            if let Some(state) = app.filesearch.as_mut() {
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
    match path.strip_prefix(root) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}
