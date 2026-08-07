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

/// One path the finder can offer: a document already in the recovery
/// store's MRU list, or a file the ignore-aware workspace walk found.
/// `in_tree`/`mru_rank` drive the ranking a later work package adds;
/// `display` is the string actually matched and shown — root-relative for
/// an in-tree path, absolute otherwise.
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

/// The recompute-over-cache chokepoint every query edit funnels through —
/// for now, an empty query lists whatever `recents`/`walk` already hold
/// (both stay empty until a later work package's `Cmd`s populate them); a
/// non-empty query has no ranking yet. `effects` is threaded through from
/// day one because a later work package's ranking also triggers a preview
/// request when the selection changes.
pub(crate) fn recompute(app: &mut App, _effects: &mut Effects) {
    let Some(state) = app.filesearch.as_mut() else {
        return;
    };
    if !state.query.trim().is_empty() {
        state.results = Vec::new();
        return;
    }
    let total = state.recents.len() + state.walk.len();
    state.results = (0..total)
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
}
