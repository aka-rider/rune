//! Tier-1 observation: a plain, owned, hand-constructible struct capturing
//! everything a checker needs to know about `App` state at one point in
//! time.
//!
//! `Snapshot` cannot express five invariants that need `effects.raw`, VFS
//! bytes/delivery history, or the triggering message (`CLIP-OSC52`, `SAVE-
//! VERBATIM`, `SAVE-CLEAN-MATCHES-DISK`, `SAVE-INFLIGHT-SM`, `QUIT-CHORD`,
//! `CONFIRM-GEN` — added by a later work package) — that data lives in
//! `crate::step::StepCtx` instead (plan Context, decision 7 `[fixes B3]`).

use std::collections::BTreeMap;
use std::ops::Range;

use rune_core::coords::DisplayRow;
use rune_core::cursor::Cursor;
use rune_core::undo::EditKind;
use rune_syntax::element::ByteRange;
use rune_tui::app::App;
use rune_tui::document::{DocumentId, ReadOnly};
use rune_tui::focus::{self, FocusTarget};
use rune_tui::footer;
use rune_tui::generation::QuitGen;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::QuitKey;
use rune_tui::layout::{self, Geometry};
use rune_tui::pane::Pane;
use rune_tui::render::{self, Cell};
use rune_tui::row_meta::{self, RowMeta};

#[derive(Clone, Debug)]
pub struct Painted {
    pub doc: DocumentId,
    pub content: String,
    pub version: u64,
    pub cursors: Vec<Cursor>,
    pub caret_visible: bool,
    pub reading_link_focus: Option<ByteRange>,
    pub cells: Vec<Vec<Cell>>,
    pub row_meta: Vec<RowMeta>,
    pub highlight_spans: Vec<(usize, usize)>,
    pub highlight_version: u64,
    pub scroll_row: DisplayRow,
    /// `view.display.total_rows()` for the shown document, or `1` when
    /// there is no view yet (a fresh document renders one empty row) —
    /// captured unconditionally, unlike `cells`/`row_meta`, so
    /// `SCROLL-IN-DOC` (`invariant/render.rs`) can check every step, not
    /// just sampled ones.
    pub total_rows: usize,
}

/// One point-in-time observation of `App`, built ONLY from its public
/// accessors (`Buffer`'s fields are private — G16 — so line bounds come
/// from `line_start`/`line_end`; `CursorSet` has no iterator, so cursors
/// come from `CursorSet::all()`, which clones).
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub content: String,
    pub version: u64,
    pub saved_version: u64,
    pub is_dirty: bool,
    pub cursors: Vec<Cursor>,
    pub line_count: usize,
    /// `Buffer::line_start(0..line_count)`.
    pub line_starts: Vec<usize>,
    /// `Buffer::line_end(0..line_count)`.
    pub line_ends: Vec<usize>,
    pub journal_pos: usize,
    pub journal_len: usize,
    pub journal_tip_strip_run: usize,
    pub save_in_flight: bool,
    pub pending_quit: Option<(QuitKey, QuitGen)>,
    pub should_quit: bool,
    pub status: String,
    /// `app.focus` — which chrome region owns the next keystroke (`pane::
    /// Pane`). Needed by `PANE-NO-BLEED` (`invariant/pane.rs`) and by the
    /// end-of-session undo/redo drive (`driver.rs`), both of which must
    /// tell an editor-focused step apart from a chrome-focused one.
    pub focus: Pane,
    pub focus_target: FocusTarget,
    /// `app.guard.is_some()` — a Guard captures every key at stage 1 of the
    /// pipeline regardless of `focus` (an error, unlike a Guard, is a
    /// non-modal log entry and captures nothing), so `PANE-NO-BLEED` and the
    /// undo/redo drive precondition both need this in addition to `focus`.
    pub modal_open: bool,
    /// `app.active` — which document the OTHER fields in this `Snapshot`
    /// describe. `PANE-NO-BLEED` needs it to tell a chrome key that
    /// switched the active document (Explorer/Tabs `Enter`, `^w`) apart
    /// from one that left it alone but still shouldn't touch its content.
    pub active: DocumentId,
    /// `app.title.text()` — the title field's current text, independent of
    /// whether the title is focused right now. No checker keys off this
    /// yet, but without it none COULD: before this field existed a
    /// `Snapshot` carried no title text at all (plan WP5.S5), so a failure
    /// report couldn't even show what the field held when a title-path
    /// invariant tripped.
    pub title_text: String,
    pub title_cursor: Cursor,
    pub title_window: Range<usize>,
    pub filesearch_query: Option<String>,
    pub projectsearch_query: Option<String>,
    pub search_draft: Option<String>,
    pub palette_query: Option<String>,
    /// `doc.read_only` — the virtual Help document (`workspace::
    /// toggle_help`) and reading view (`ReadOnly::Reading`) are both live
    /// paths to a read-only document; `PASTE-VERBATIM` needs this to tell
    /// "production correctly refused the edit" apart from "production
    /// silently dropped it" (a paste into a read-only document is the
    /// former, by design).
    pub read_only: ReadOnly,
    pub painted: Painted,
    /// `layout::geometry(app.frame_area(),
    /// app)` — every rect the frame is built from, captured once per step so
    /// `LAYOUT-FITS` (`invariant/pane.rs`) can check it as a plain function
    /// of `next` alone, with no live `App` reach-back.
    pub geometry: Geometry,
    /// `app.modal`'s `Guard` variant, if one is up — the document it names
    /// and its `GuardKind` (plan WP2). `None` for every other modal state
    /// (`Error`, or no modal at all) — no checker here needs to tell those
    /// two apart, only "is a Guard, specifically, up right now".
    pub guard: Option<(DocumentId, GuardKind)>,
    /// `app.quit_intent`'s wait set, as a plain sorted `Vec` (plan WP2) —
    /// `None` when no quit-save fan-out is outstanding, `Some(vec![])` is
    /// never observed in practice (an empty map is retired to `None` the
    /// same tick it empties, `materialize_ack::retire_quit_wait`'s own
    /// invariant) but is not ruled out structurally, so a checker should
    /// treat an empty `Some` the same as `None` rather than assuming it
    /// can't happen.
    pub quit_intent_pending: Option<Vec<(DocumentId, u64)>>,
    /// Every open document's own `Document::is_dirty` — `Snapshot.is_dirty`
    /// above is active-document-only, so a quit/close-guard invariant that
    /// needs to know whether some OTHER, inactive document is dirty has no
    /// other way to see it.
    pub dirty_by_doc: BTreeMap<DocumentId, bool>,
    /// `Document::save_in_flight` for every open document, the same
    /// per-document shape as `dirty_by_doc` above. `QUIT-CHORD` needs this
    /// to recognize a quit-save entry retiring through a materialize ack
    /// that isn't tagged `MsgTag::SaveDone` (the store-backed `Msg::Db`
    /// route into `handle_materialize_ack` -> `quit_if_pending` completes
    /// the exact same save lifecycle chokepoint, just via a different
    /// message shape the fuzz driver has no document that can construct
    /// yet) — a true->false transition here for a document that was in the
    /// prior step's quit-wait set is the save lifecycle's OWN signal that
    /// its save actually completed, independent of which message carried
    /// the ack.
    pub save_in_flight_by_doc: BTreeMap<DocumentId, bool>,
    /// Every open document's own `saved_version`, the same per-document
    /// shape as `dirty_by_doc` above — `Snapshot.saved_version` is
    /// active-document-only, so a checker asking "did THIS document's save
    /// commit" would otherwise be comparing two different documents'
    /// versions across a step that changed the active document.
    pub saved_version_by_doc: BTreeMap<DocumentId, u64>,
    /// Whether `app.merge` is `Active`/`Pending` right now (plan WP7.S1) —
    /// a lean projection of `MergeState`, not the enum itself: no checker
    /// here needs the working-form `pairs` bodies, only
    /// whether an attempt is live, which document it names, and how many
    /// blocks remain unresolved.
    pub merge_active: bool,
    pub merge_pending: bool,
    /// `MergeState::doc()` — `None` only for `Inactive`.
    pub merge_doc: Option<DocumentId>,
    /// `MergeState::unresolved_count()` — `0` outside `Active`.
    pub merge_unresolved: usize,
    /// Every open document's own `display_name`, the same per-document
    /// shape as `dirty_by_doc` above — `MergeState::Inactive` must never
    /// leave a stale `"editor <-> disk"` retitle behind on ANY document,
    /// not just whichever one happens to be active.
    pub display_name_by_doc: BTreeMap<DocumentId, Option<String>>,
    /// `app.active_doc().last_sync` — the ACTIVE document's last known sync
    /// classification, exactly what the divergence chrome (footer banner,
    /// `⇄` marker, conditional `^M` hint) and `merge::begin`'s fast
    /// pre-check read. `MERGE-NO-INSTANT-REDIVERGENCE`
    /// (`invariant/merge.rs`'s stateful tracker) keys off it to catch a
    /// probe re-classifying a just-reconciled document `Diverged` with the
    /// disk untouched — the infinite re-merge-prompt loop.
    pub active_last_sync: Option<rune_db::SyncKind>,
    /// `rune_tui::messages::posts` — the message log's own monotonic post
    /// counter, not re-derived from `status`: two consecutive posts of
    /// identical text (the same merge-key hint fired by two different
    /// unbound keys in a row) leave `Snapshot.status` looking unchanged
    /// even though a new row landed in the pane, so a checker that treats
    /// "was a message posted" as "did `status` change" is blind to that
    /// case. `MERGE-KEY-FEEDBACK` needs the distinction directly.
    pub message_posts: u64,
    pub nav_places: Vec<(DocumentId, usize, bool)>,
    pub nav_current: usize,
    pub buffer_len_by_doc: BTreeMap<DocumentId, usize>,
}

/// `Snapshot.status`'s builder: the footer's own text, plus the message
/// log's newest entry (transient messages live in the log now, not in the
/// footer), joined by `" | "` so every existing invariant/tripwire reading
/// `Snapshot.status` for message text keeps seeing it. Footer text alone
/// when the log is empty.
fn fuzz_status(app: &App) -> String {
    let footer_text = footer::footer_text(app);
    match rune_tui::messages::newest_text(app) {
        Some(newest) => format!("{footer_text} | {newest}"),
        None => footer_text,
    }
}

impl Snapshot {
    /// Captures the current `App` state. Deliberately does NOT call
    /// `app.sync_view()` itself (CODE-REVIEW.md rune-fuzz finding 1): every
    /// caller already synced immediately beforehand (`driver::run`'s setup,
    /// or `step_and_check`'s `run_update_catching_panic`, both of which end
    /// in exactly one `sync_view()` call) — an extra unconditional sync
    /// here used to put `SYNC-IDEMPOTENT`'s own later re-sync a call too
    /// late to ever see the state right after `update`, three syncs deep
    /// before the checker's comparison ever ran. `&mut App` is kept (not
    /// `&App`) only because `with_cells`'s `render::build_rows` call
    /// borrows through the same active-document access pattern as the rest
    /// of this function.
    pub fn capture(app: &mut App, with_cells: bool) -> Snapshot {
        let buf = &app.active_doc().buffer;
        let line_count = buf.line_count();
        let mut line_starts = Vec::with_capacity(line_count);
        let mut line_ends = Vec::with_capacity(line_count);
        for n in 0..line_count {
            // `n` ranges over `0..line_count`, so both are always `Some`.
            line_starts.push(buf.line_start(n).unwrap_or(0));
            line_ends.push(buf.line_end(n).unwrap_or(0));
        }

        let (cells, row_meta) = if with_cells {
            match &app.shown_doc().view {
                Some(view) => (
                    render::build_rows(app, render::RowSource::Shown, view),
                    row_meta::row_meta(view, app),
                ),
                None => (Vec::new(), Vec::new()),
            }
        } else {
            (Vec::new(), Vec::new())
        };

        let guard = app
            .guard
            .as_ref()
            .map(|prompt| (prompt.doc, prompt.kind.clone()));
        let quit_intent_pending = app
            .quit
            .fan_out()
            .map(|intent| intent.pending.iter().map(|(&id, &v)| (id, v)).collect());
        let doc_ids: Vec<DocumentId> = app.documents.keys().copied().collect();
        let mut dirty_by_doc = BTreeMap::new();
        let mut save_in_flight_by_doc = BTreeMap::new();
        let mut display_name_by_doc = BTreeMap::new();
        let mut saved_version_by_doc = BTreeMap::new();
        let mut buffer_len_by_doc = BTreeMap::new();
        for doc_id in doc_ids {
            if let Some(d) = app.doc(doc_id) {
                dirty_by_doc.insert(doc_id, d.is_dirty());
                save_in_flight_by_doc.insert(doc_id, d.save_in_flight());
                display_name_by_doc.insert(doc_id, d.display_name.clone());
                saved_version_by_doc.insert(doc_id, d.saved_version);
                buffer_len_by_doc.insert(doc_id, d.buffer.content().len());
            }
        }
        let nav_places = app
            .nav_history
            .places()
            .iter()
            .map(|place| {
                let on_char_boundary = app
                    .doc(place.doc)
                    .is_none_or(|d| d.buffer.content().is_char_boundary(place.offset));
                (place.doc, place.offset, on_char_boundary)
            })
            .collect();
        let nav_current = app.nav_history.index();
        let merge_active = matches!(app.merge, rune_tui::merge::MergeState::Active { .. });
        let merge_pending = matches!(app.merge, rune_tui::merge::MergeState::Pending { .. });
        let merge_doc = app.merge.doc();
        let merge_unresolved = app.merge.unresolved_count();

        let shown = app.shown_doc();
        // The whole document, not a viewport window: a checker must see
        // every span the renderer could paint at any scroll position, and
        // `visible_spans` clamps and sorts identically whatever window it is
        // handed.
        let highlight_spans =
            rune_tui::highlight::visible_spans(shown, 0..shown.buffer.content().len())
                .into_iter()
                .map(|(range, _scope)| (range.start, range.end))
                .collect();
        let painted = Painted {
            doc: app.shown(),
            content: shown.buffer.content().to_string(),
            version: shown.buffer.version(),
            cursors: shown.cursors.all().to_vec(),
            caret_visible: shown.has_insertion_point(),
            reading_link_focus: shown.reading_link_focus,
            cells,
            row_meta,
            highlight_spans,
            highlight_version: shown.highlight.version,
            scroll_row: shown.viewport.scroll_row,
            total_rows: shown.view.as_ref().map_or(1, |v| v.display.total_rows()),
        };
        let doc = app.active_doc();
        let geometry = layout::geometry(app.frame_area(), app);
        Snapshot {
            content: doc.buffer.content().to_string(),
            version: doc.buffer.version(),
            saved_version: doc.saved_version,
            is_dirty: app.is_dirty(),
            cursors: doc.cursors.all().to_vec(),
            line_count,
            line_starts,
            line_ends,
            journal_pos: doc.journal.pos(),
            journal_len: doc.journal.len(),
            journal_tip_strip_run: doc
                .journal
                .steps()
                .get(..doc.journal.pos())
                .unwrap_or_default()
                .iter()
                .rev()
                .take_while(|step| step.kind == EditKind::StripTrailingWhitespace)
                .count(),
            save_in_flight: doc.save_in_flight(),
            pending_quit: match app.quit {
                rune_tui::app::QuitNegotiation::ConfirmArmed(key, generation) => {
                    Some((key, generation))
                }
                rune_tui::app::QuitNegotiation::Idle
                | rune_tui::app::QuitNegotiation::SaveFanOut(_) => None,
            },
            should_quit: app.should_quit,
            status: fuzz_status(app),
            focus: app.focus(),
            focus_target: focus::target(app),
            modal_open: app.guard.is_some(),
            active: app.active,
            title_text: app.title.text().to_string(),
            title_cursor: app.title.field().cursor(),
            title_window: app.title.window(),
            filesearch_query: app.filesearch().map(|state| state.query.clone()),
            projectsearch_query: app.projectsearch().map(|state| state.query.clone()),
            search_draft: app.search_draft().map(str::to_string),
            palette_query: app.palette().map(|state| state.field.text().to_string()),
            read_only: doc.read_only,
            painted,
            geometry,
            guard,
            quit_intent_pending,
            dirty_by_doc,
            save_in_flight_by_doc,
            saved_version_by_doc,
            merge_active,
            merge_pending,
            merge_doc,
            merge_unresolved,
            display_name_by_doc,
            active_last_sync: doc.last_sync,
            message_posts: rune_tui::messages::posts(app),
            nav_places,
            nav_current,
            buffer_len_by_doc,
        }
    }
}
