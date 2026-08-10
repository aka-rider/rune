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

use ratatui::layout::Rect;
use rune_core::cursor::Cursor;
use rune_tui::app::App;
use rune_tui::document::{DocumentId, ReadOnly};
use rune_tui::footer;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::QuitKey;
use rune_tui::layout::{self, Geometry};
use rune_tui::pane::Pane;
use rune_tui::render::{self, Cell};
use rune_tui::row_meta::{self, RowMeta};

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
    pub save_in_flight: bool,
    pub pending_quit: Option<(QuitKey, u32)>,
    pub should_quit: bool,
    pub status: String,
    /// `app.focus` — which chrome region owns the next keystroke (`pane::
    /// Pane`). Needed by `PANE-NO-BLEED` (`invariant/pane.rs`) and by the
    /// end-of-session undo/redo drive (`driver.rs`), both of which must
    /// tell an editor-focused step apart from a chrome-focused one.
    pub focus: Pane,
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
    /// `doc.read_only` — the virtual Help document (`workspace::
    /// toggle_help`, reachable now that `F1` is in `arb_any_keycode`,
    /// CODE-REVIEW.md rune-fuzz finding 9), reading view (`⌃P`,
    /// `ReadOnly::Reading`), and a not-yet-committed preview (`ReadOnly::
    /// Preview`) are all live paths to a read-only document; `PASTE-
    /// VERBATIM` needs this to tell "production correctly refused the
    /// edit" apart from "production silently dropped it" (a paste into a
    /// read-only document is the former, by design — `Document::read_only`
    /// is no longer dead code once any of these paths exists). The full
    /// variant, not a collapsed `bool`: a checker that only ever asks "is
    /// this read-only at all" still gets that from `!matches!(..,
    /// ReadOnly::No)`, but a future checker that needs to tell `Preview`
    /// apart from `Reading`/`Always` now can, rather than the enum being
    /// invisible to every session the fuzzer drives.
    pub read_only: ReadOnly,
    /// `app.active_doc().has_insertion_point()` — the production predicate itself,
    /// not a re-derivation from `focus`/`modal_open`/`read_only`, so
    /// `CUR-NO-CARET-HIDDEN` cannot pass by duplicating the very logic it is
    /// meant to police.
    pub caret_visible: bool,
    /// `render::build_rows(view, app)`. Empty when not sampled (G19: the
    /// display pipeline runs on every `sync_view()`, dominating debug-build
    /// runtime — later work packages sample this rather than paying for it
    /// every step).
    pub cells: Vec<Vec<Cell>>,
    /// `row_meta::row_meta(view, app)` (plan WP5.S1) — table membership for
    /// each of `cells`' own rows, same index, same sampling (empty
    /// whenever `cells` is). Gives `TABLE-ROW-WIDTH`/
    /// `TABLE-SYNTHETIC-DECORATIVE` the table/border signal `cells` alone
    /// cannot express.
    pub row_meta: Vec<RowMeta>,
    /// `highlight::visible_spans` over the WHOLE document, as plain
    /// `(start, end)` byte ranges — the `ScopeId` tag isn't needed by any
    /// checker here, so it's dropped rather than carried.
    ///
    /// Deliberately the same function the renderer calls, not the stored
    /// state behind it. Once every code region is tree-backed, the stored
    /// span channel is empty for most documents, and a projection reading it
    /// would leave `HL-CLAMPED`/`HL-STALE-DROP` passing while testing
    /// nothing. Projecting the query instead keeps both invariants pointed
    /// at exactly what a user would see, and extends them to the whole-file
    /// path they never reached.
    pub highlight_spans: Vec<(usize, usize)>,
    /// `doc.highlight.version` — the buffer version the region state
    /// `highlight_spans` was queried from describes. Production's own
    /// `schedule_highlight` compares it against `doc.buffer.version()` as
    /// its "regions still describe the live buffer" test. No checker keys
    /// off it any more, now that the clamp lives in the query and staleness
    /// is therefore never an excuse for a bad span; it is carried so a
    /// failure report can still show whether the regions were current.
    pub highlight_version: u64,
    /// `layout::geometry(Rect::new(0, 0, app.frame_width, app.frame_height),
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
    /// Every open document's own dirty cache, re-derived (via `App::
    /// recompute_dirty`, the one public seam onto the same chokepoint
    /// `materialize_ack::is_dirty_now` uses internally) rather than read
    /// stale — `Snapshot.is_dirty` above is active-document-only, so a
    /// quit/close-guard invariant that needs to know whether some OTHER,
    /// inactive document is dirty has no other way to see it.
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
    /// Whether `app.merge` is `Active`/`Pending` right now (plan WP7.S1) —
    /// a lean projection of `MergeState`, not the enum itself: no checker
    /// here needs the working-form `conflicts`/`blocks` bodies, only
    /// whether an attempt is live, which document it names, and how many
    /// blocks remain unresolved.
    pub merge_active: bool,
    pub merge_pending: bool,
    /// `MergeState::doc()` — `None` only for `Inactive`.
    pub merge_doc: Option<DocumentId>,
    /// `MergeState::unresolved_count()` — `0` outside `Active`.
    pub merge_unresolved: usize,
    /// `doc.viewport.scroll_row` — the active document's own scroll
    /// position, needed alongside `content`/`version`/`cursors` so a
    /// checker can tell "this key moved the viewport" apart from "this key
    /// did nothing": the merge-mode no-silent-swallow invariant treats a
    /// key dispatched while the resolver is `Active` as a violation unless
    /// it changed the buffer, cursors, scroll position, merge state, or the
    /// status line — scroll is the one of those five this field alone
    /// supplies.
    pub scroll_row: usize,
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
            match &app.active_doc().view {
                Some(view) => (
                    render::build_rows(app, app.active_doc(), Some(app.active), view),
                    row_meta::row_meta(view, app),
                ),
                None => (Vec::new(), Vec::new()),
            }
        } else {
            (Vec::new(), Vec::new())
        };

        // Computed BEFORE `doc` below borrows `app` immutably for the rest
        // of this function: `recompute_dirty` needs `&mut App`, so it must
        // run while no other borrow of `app` is still alive.
        let guard = app
            .guard
            .as_ref()
            .map(|prompt| (prompt.doc, prompt.kind.clone()));
        let quit_intent_pending = app
            .quit_intent
            .as_ref()
            .map(|intent| intent.pending.iter().map(|(&id, &v)| (id, v)).collect());
        let doc_ids: Vec<DocumentId> = app.documents.keys().copied().collect();
        let mut dirty_by_doc = BTreeMap::new();
        let mut save_in_flight_by_doc = BTreeMap::new();
        let mut display_name_by_doc = BTreeMap::new();
        for doc_id in doc_ids {
            app.recompute_dirty(doc_id);
            if let Some(d) = app.doc(doc_id) {
                dirty_by_doc.insert(doc_id, d.is_dirty());
                save_in_flight_by_doc.insert(doc_id, d.save_in_flight());
                display_name_by_doc.insert(doc_id, d.display_name.clone());
            }
        }
        let merge_active = matches!(app.merge, rune_tui::merge::MergeState::Active { .. });
        let merge_pending = matches!(app.merge, rune_tui::merge::MergeState::Pending { .. });
        let merge_doc = app.merge.doc();
        let merge_unresolved = app.merge.unresolved_count();

        let doc = app.active_doc();
        // The whole document, not a viewport window: a checker must see
        // every span the renderer could paint at any scroll position, and
        // `visible_spans` clamps and sorts identically whatever window it is
        // handed.
        let highlight_spans =
            rune_tui::highlight::visible_spans(doc, 0..doc.buffer.content().len())
                .into_iter()
                .map(|(range, _scope)| (range.start, range.end))
                .collect();
        let highlight_version = doc.highlight.version;
        let geometry = layout::geometry(Rect::new(0, 0, app.frame_width, app.frame_height), app);
        let scroll_row = doc.viewport.scroll_row;
        Snapshot {
            content: doc.buffer.content().to_string(),
            version: doc.buffer.version(),
            saved_version: doc.saved_version,
            is_dirty: app.is_dirty(),
            cursors: doc.cursors.all(),
            line_count,
            line_starts,
            line_ends,
            journal_pos: doc.journal.pos(),
            journal_len: doc.journal.len(),
            save_in_flight: doc.save_in_flight(),
            pending_quit: app.pending_quit,
            should_quit: app.should_quit,
            status: fuzz_status(app),
            focus: app.focus(),
            modal_open: app.guard.is_some(),
            active: app.active,
            title_text: app.title.text().to_string(),
            read_only: doc.read_only,
            caret_visible: doc.has_insertion_point(),
            cells,
            row_meta,
            highlight_spans,
            highlight_version,
            geometry,
            guard,
            quit_intent_pending,
            dirty_by_doc,
            save_in_flight_by_doc,
            merge_active,
            merge_pending,
            merge_doc,
            merge_unresolved,
            scroll_row,
            display_name_by_doc,
            active_last_sync: doc.last_sync,
            message_posts: rune_tui::messages::posts(app),
        }
    }
}
