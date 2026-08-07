//! `FileSearchState` — the fuzzy file finder overlay's own state, shaped
//! after the in-file search bar's own (`App::search`, `search/mod.rs`):
//! present on `App` only while the finder is open, never a `Pane`
//! (`FocusTarget::FileSearch`, `focus.rs`) and, while open, the underlying
//! chrome `Pane` stays `Explorer`. This module owns open/close/cancel and
//! the recompute chokepoint (render never filters or ranks); keystroke
//! handling lives in the [`keys`] submodule, the query row and result list
//! render through `render::filesearch`.

use std::path::PathBuf;

use nucleo_matcher::{Config, Matcher};

use crate::app::App;
use crate::document::DocumentId;
use crate::listnav;
use crate::pane::Pane;
use crate::runtime::Effects;
use crate::workspace;

pub(crate) mod keys;

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
    pub recents: Vec<Candidate>,
    pub walk: Vec<Candidate>,
    pub walk_pending: bool,
    pub walk_truncated: bool,
    pub results: Vec<ResultRow>,
    // Scoring/highlighting are wired up once candidates actually exist; kept
    // here now so a whole finder session amortizes one matcher's internal
    // scratch allocation instead of minting a fresh one per keystroke.
    #[allow(dead_code)]
    matcher: Matcher,
    #[allow(dead_code)]
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
    app.filesearch = Some(FileSearchState {
        query: String::new(),
        nav: listnav::List { cursor: 0, top: 0 },
        generation,
        return_to,
        recents: Vec::new(),
        walk: Vec::new(),
        walk_pending: false,
        walk_truncated: false,
        results: Vec::new(),
        matcher: Matcher::new(Config::DEFAULT.match_paths()),
        charbuf: Vec::new(),
    });
    app.set_focus_pane(Pane::Explorer, effects);

    if let Some(db) = app.db.as_ref() {
        effects
            .cmds
            .push(crate::runtime::load_filesearch_recents_cmd(
                db.store.reader_query(),
                std::sync::Arc::clone(&app.vfs),
                resolve_root(app),
                generation,
            ));
    }
}

/// The finder's own walk/recents root (plan A4): `app.root` when it's set,
/// else the Explorer's own fallback ladder (`explorer::initial_root`) —
/// resolved fresh on every open, never cached, since the active document
/// (and so the ladder's own first rung) can have changed since the finder
/// last closed.
fn resolve_root(app: &App) -> PathBuf {
    if app.root.as_os_str().is_empty() {
        crate::explorer::initial_root(app)
    } else {
        app.root.clone()
    }
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
/// never physically merged into one `Vec`.
pub(crate) fn candidate_at(state: &FileSearchState, idx: usize) -> Option<&Candidate> {
    match idx.checked_sub(state.recents.len()) {
        None => state.recents.get(idx),
        Some(walk_idx) => state.walk.get(walk_idx),
    }
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
/// `recents`/`walk` load) funnels through — a non-empty query has no
/// ranking yet (a later work package's job); an empty query lists in-tree
/// recents (MRU order) then out-of-tree recents (MRU order), then `walk`
/// files in whatever order they arrived (empty until a later work
/// package's `Cmd` populates it), capped at [`RESULT_CAP`]. `effects` is
/// threaded through from day one because a later work package's ranking
/// also triggers a preview request when the selection changes.
pub(crate) fn recompute(app: &mut App, _effects: &mut Effects) {
    let Some(state) = app.filesearch.as_mut() else {
        return;
    };
    if !state.query.trim().is_empty() {
        state.results = Vec::new();
        return;
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
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

    /// The load-bearing ordering: opening on a fresh app whose left column
    /// was never shown must still land the finder open with the Explorer
    /// focused — this pins that `app.filesearch` is assigned BEFORE
    /// `set_focus_pane` runs.
    #[test]
    fn open_on_a_never_shown_left_column_still_opens_and_focuses_explorer() {
        let mut app = app();
        assert!(!app.splits.left.is_shown(), "test setup: column hidden");
        let mut effects = Effects::default();

        open(&mut app, &mut effects);

        assert!(app.filesearch.is_some());
        assert_eq!(app.focus(), Pane::Explorer);
    }

    #[test]
    fn open_is_a_no_op_when_already_open() {
        let mut app = app();
        let mut effects = Effects::default();
        open(&mut app, &mut effects);
        let generation_before = app.filesearch.as_ref().map(|s| s.generation);

        open(&mut app, &mut effects);

        assert_eq!(
            app.filesearch.as_ref().map(|s| s.generation),
            generation_before
        );
    }

    #[test]
    fn cancel_restores_the_document_that_was_active_before_open() {
        let mut app = app();
        let second = app.open_document(Buffer::new("second"));
        crate::workspace::switch_to(&mut app, second);
        let mut effects = Effects::default();

        open(&mut app, &mut effects);
        cancel(&mut app, &mut effects);

        assert!(app.filesearch.is_none());
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
        let return_to = app.filesearch.as_ref().map(|s| s.return_to).expect("open");
        assert_eq!(return_to, second, "test setup: return_to names `second`");

        crate::workspace::request_close(&mut app, second, &mut effects);
        assert!(app.doc(second).is_none(), "test setup: closed for real");

        cancel(&mut app, &mut effects);

        assert!(app.filesearch.is_none());
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

    /// Pins the plan's own empty-query ordering: in-tree recents first (MRU
    /// order preserved), then out-of-tree recents (MRU order preserved) —
    /// even though the reply itself arrives with the two interleaved.
    #[test]
    fn recents_loaded_orders_in_tree_before_out_of_tree_preserving_mru() {
        let mut app = app();
        let mut effects = Effects::default();
        open(&mut app, &mut effects);
        let generation = app.filesearch.as_ref().expect("open").generation;

        let recents = vec![
            candidate("/outside/z.md", false),
            candidate("/root/a.md", true),
            candidate("/root/b.md", true),
        ];

        handle_recents_loaded(&mut app, generation, Ok(recents), &mut effects);

        let state = app.filesearch.as_ref().expect("still open");
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

    /// A close-then-reopen mints a fresh generation (`open`'s own contract);
    /// a reply still carrying the OLD generation must never populate the
    /// new session's `recents`.
    #[test]
    fn recents_loaded_reply_is_dropped_after_a_close_then_reopen_mints_a_new_generation() {
        let mut app = app();
        let mut effects = Effects::default();
        open(&mut app, &mut effects);
        let stale_generation = app.filesearch.as_ref().expect("open").generation;
        close(&mut app);
        open(&mut app, &mut effects);
        let fresh_generation = app.filesearch.as_ref().expect("reopen").generation;
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
            app.filesearch
                .as_ref()
                .expect("still open")
                .recents
                .is_empty(),
            "a stale reply must never populate the fresh session's recents"
        );
    }

    #[test]
    fn recents_loaded_err_reply_posts_a_message_and_leaves_recents_empty() {
        let mut app = app();
        let mut effects = Effects::default();
        open(&mut app, &mut effects);
        let generation = app.filesearch.as_ref().expect("open").generation;

        handle_recents_loaded(&mut app, generation, Err("boom".to_string()), &mut effects);

        assert!(
            app.filesearch
                .as_ref()
                .expect("still open")
                .recents
                .is_empty()
        );
        assert_eq!(
            crate::messages::newest_text(&app),
            Some("recent files not loaded: boom")
        );
    }
}
