//! Tier-1 observation: a plain, owned, hand-constructible struct capturing
//! everything a checker needs to know about `App` state at one point in
//! time. Go analogue: its own fuzz `Snapshot`.
//!
//! `Snapshot` cannot express five invariants that need `effects.raw`, VFS
//! bytes/delivery history, or the triggering message (`CLIP-OSC52`, `SAVE-
//! VERBATIM`, `SAVE-CLEAN-MATCHES-DISK`, `SAVE-INFLIGHT-SM`, `QUIT-CHORD`,
//! `CONFIRM-GEN` — added by a later work package) — that data lives in
//! `crate::step::StepCtx` instead (plan Context, decision 7 `[fixes B3]`).

use rune_core::cursor::Cursor;
use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::footer;
use rune_tui::keymap::QuitKey;
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
    /// `focus` (`app.rs:457-461`), so `PANE-NO-BLEED` and the undo/redo
    /// drive precondition both need this in addition to `focus`.
    pub modal_open: bool,
    /// `app.active` — which document the OTHER fields in this `Snapshot`
    /// describe. `PANE-NO-BLEED` needs it to tell a chrome key that
    /// switched the active document (Explorer/Tabs `Enter`, `^w`) apart
    /// from one that left it alone but still shouldn't touch its content.
    pub active: DocumentId,
    /// `render::build_rows(view, app)`. Empty when not sampled (G19: the
    /// display pipeline runs on every `sync_view()`, dominating debug-build
    /// runtime — later work packages sample this rather than paying for it
    /// every step). Deliberately NOT `read_only` (`Editor::read_only` is
    /// dead in Phase 1 — G18).
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
}

impl Snapshot {
    /// Captures the current `App` state after re-running `app.sync_view()`
    /// — idempotent and cheap when nothing changed (`Editor::sync`'s own
    /// docs; G6 proves it's a genuine fixpoint), so calling it again here
    /// even when the driver already synced this step costs nothing and
    /// guarantees `app.view` is fresh before `cells` is built from it.
    /// `&mut App`, not `&App`, because `sync_view` requires it.
    pub fn capture(app: &mut App, with_cells: bool) -> Snapshot {
        app.sync_view();

        let buf = &app.active_doc().buffer;
        let line_count = buf.line_count();
        let mut line_starts = Vec::with_capacity(line_count);
        let mut line_ends = Vec::with_capacity(line_count);
        for n in 0..line_count {
            line_starts.push(buf.line_start(n));
            line_ends.push(buf.line_end(n));
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
            cells,
            row_meta,
            highlight_spans,
            highlight_version,
        }
    }
}
