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

        let cells = if with_cells {
            match &app.active_doc().view {
                Some(view) => render::build_rows(view, app),
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let doc = app.active_doc();
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
        }
    }
}
