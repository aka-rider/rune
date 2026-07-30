//! Tier-1 observation: a plain, owned, hand-constructible struct capturing
//! everything a checker needs to know about `App` state at one point in
//! time. Go analogue: its own fuzz `Snapshot`.
//!
//! `Snapshot` cannot express five invariants that need `effects.raw`, VFS
//! bytes/delivery history, or the triggering message (`CLIP-OSC52`, `SAVE-
//! VERBATIM`, `SAVE-CLEAN-MATCHES-DISK`, `SAVE-INFLIGHT-SM`, `QUIT-CHORD`,
//! `CONFIRM-GEN` — added by a later work package) — that data lives in
//! `crate::step::StepCtx` instead (plan Context, decision 7 `[fixes B3]`).

use ratatui::layout::Rect;
use rune_core::cursor::Cursor;
use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::footer;
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
    /// `app.modal.is_some()` — a modal (`Modal::Error`/`Modal::Guard`)
    /// captures every key at stage 1 of the pipeline regardless of
    /// `focus`, so `PANE-NO-BLEED` and the undo/redo drive precondition
    /// both need this in addition to `focus`.
    pub modal_open: bool,
    /// `app.active` — which document the OTHER fields in this `Snapshot`
    /// describe. `PANE-NO-BLEED` needs it to tell a chrome key that
    /// switched the active document (Explorer/Tabs `Enter`, `^w`) apart
    /// from one that left it alone but still shouldn't touch its content.
    pub active: DocumentId,
    /// `app.active_doc().read_only` — the virtual Help document
    /// (`workspace::toggle_help`, reachable now that `F1` is in
    /// `arb_any_keycode`, CODE-REVIEW.md rune-fuzz finding 9) is the one
    /// live `read_only` document Phase 1 actually mints; `PASTE-VERBATIM`
    /// needs this to tell "production correctly refused the edit" apart
    /// from "production silently dropped it" (a paste into a read-only
    /// document is the former, by design — `Document::read_only` is no
    /// longer dead code once Help exists).
    pub read_only: bool,
    /// `app.active_doc().shows_caret()` — the production predicate itself,
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
    /// `doc.highlight.spans`, as plain `(start, end)` byte ranges — the
    /// `ScopeId` tag isn't needed by any checker here, so it's dropped
    /// rather than carried (plan WP7.S7). `HL-CLAMPED`/`HL-STALE-DROP` key
    /// off this.
    pub highlight_spans: Vec<(usize, usize)>,
    /// `doc.highlight.version` — the buffer version `highlight_spans`
    /// describes. Production's own `schedule_highlight` (`highlight.rs`)
    /// uses `doc.highlight.version == doc.buffer.version()` as its "spans
    /// still describe the live buffer" test; `HL-CLAMPED` uses the SAME
    /// comparison against `Snapshot.version` — a stale (mismatched) tag
    /// means the stored spans are KNOWN to describe a past version and are
    /// deliberately left in place (WP5.S4's `[R2]`, "stale colours, never
    /// no colours"), safely clamped only at the render layer's window
    /// boundary (`render::overlay::apply_highlight_spans`), not by
    /// `HighlightState` itself.
    pub highlight_version: u64,
    /// `layout::geometry(Rect::new(0, 0, app.frame_width, app.frame_height),
    /// app)` — every rect the frame is built from, captured once per step so
    /// `LAYOUT-FITS` (`invariant/pane.rs`) can check it as a plain function
    /// of `next` alone, with no live `App` reach-back.
    pub geometry: Geometry,
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
                Some(view) => (render::build_rows(view, app), row_meta::row_meta(view, app)),
                None => (Vec::new(), Vec::new()),
            }
        } else {
            (Vec::new(), Vec::new())
        };

        let doc = app.active_doc();
        let highlight_spans = doc
            .highlight
            .spans
            .iter()
            .map(|(range, _scope)| (range.start, range.end))
            .collect();
        let highlight_version = doc.highlight.version;
        let geometry = layout::geometry(Rect::new(0, 0, app.frame_width, app.frame_height), app);
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
            save_in_flight: doc.save_in_flight,
            pending_quit: app.pending_quit,
            should_quit: app.should_quit,
            status: footer::footer_text(app),
            focus: app.focus,
            modal_open: app.modal.is_some(),
            active: app.active,
            read_only: doc.read_only,
            caret_visible: doc.shows_caret(),
            cells,
            row_meta,
            highlight_spans,
            highlight_version,
            geometry,
        }
    }
}
