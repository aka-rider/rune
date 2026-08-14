//! The view/scroll/settle half of `Document` (split out of `document.rs`
//! for the 500-line-file budget): `view` (the pure sync query), `sync_catalogue` (its width-free
//! subset), `scroll_to_cursor`/`snap_cursor_to_row` (the single writer of
//! `viewport.scroll_row`), and `sync` (the fixed per-message/per-batch
//! sequence that ties them together). None of this depends on anything
//! declared only in `document.rs`'s own `impl Document` block — it reads and
//! writes the same public fields any other module would.

use rune_core::coords::{DisplayRow, WrapPoint, WrapRow};
use rune_core::cursor::Cursor;
use rune_md::element::doc::ViewSnapshots;

use super::Document;
use crate::viewport::ScrollMode;

impl Document {
    /// The pure QUERY half of the per-message sync sequence: `sync_content`
    /// iff version changed -> `set_width`
    /// -> `sync_cursors` -> `snapshot`. Deliberately does NOT touch
    /// `viewport.scroll_row` — see `scroll_to_cursor`'s docs (separating the
    /// snapshot-returning query from the scroll mutation removes the
    /// double-write/double-computation `sync` used to cause).
    ///
    /// Idempotent/cheap when nothing changed — `sync_content`/
    /// `sync_cursors` are no-ops in that case (reveal must
    /// never bump the buffer version) — so `commands::nav`/`commands::edit`
    /// call this freely, more than once per message batch, to get
    /// Buffer<->Syntax<->Wrap coordinate conversions that reflect the
    /// CURRENT `Document` fields (in particular a `Resize` already applied
    /// earlier in the same batch — see their module docs) before computing
    /// a new cursor position.
    pub fn view(&mut self) -> ViewSnapshots {
        self.doc.set_reveal_mode(self.reveals_under_cursor().into());
        self.sync_catalogue();
        self.doc.set_icons(self.icons.clone());
        self.doc.set_width(self.viewport.width);
        if let Some(image) = self.image() {
            let (width, rows) = match &image.status {
                crate::graphics::ImageStatus::Live { cells, .. } => (cells.cols, cells.rows),
                _ => (0, crate::render::image::INFO_CARD_ROWS),
            };
            self.doc.set_image_document_dims(width, rows);
        }
        // The inline embed footprints `sync_embeds`/
        // `handle_embed_decoded` have computed so far — empty (and so a
        // no-op past `set_embed_dims`'s own `dims != self.images` guard)
        // for every document that isn't a markdown one with at least one
        // decoded embed.
        let embed_dims = self
            .embeds()
            .map(crate::graphics::EmbedSet::to_image_dims)
            .unwrap_or_default();
        self.doc.set_embed_dims(embed_dims);
        self.doc.sync_cursors(&self.buffer, &self.cursors);
        self.doc.snapshot(&self.buffer)
    }

    /// The narrower, WIDTH-FREE half of `view()`'s parse step: re-syncs the
    /// comrak parse and rebuilds `catalogue`
    /// from it, without `view()`'s width-dependent wrap pass or cursor/
    /// snapshot work. `navigate::land_anchor` needs a just-opened target
    /// document's catalogue to find an anchor's heading BEFORE that
    /// document is necessarily ever on screen (no viewport width to wrap
    /// against yet) — exposed here, the one chokepoint `view()` itself
    /// calls into, rather than re-inlining these same two lines at that
    /// call site where they'd be free to drift from this sequence.
    pub fn sync_catalogue(&mut self) {
        let built_before = self.doc.built_version();
        self.doc.sync_content(&self.buffer);
        // The catalogue is derived solely from buffer content + blocks, so
        // it only needs rebuilding when `sync_content` actually reparsed (a
        // real content edit, or the very first call) — not on every call,
        // which commands may make several times per message batch.
        if self.doc.built_version() != built_before {
            self.catalogue =
                rune_md::catalogue::catalogue(self.buffer.content(), self.doc.blocks());
        }
    }

    /// Scrolls the viewport so the PRIMARY cursor's current row is visible.
    /// The single writer of `viewport.scroll_row` (review finding F4: "no
    /// shadow state" — a value has exactly one writer). Callers that only
    /// need coordinate conversions (`commands::nav`/`commands::edit`) must
    /// use `view()` instead and never call this themselves: calling it
    /// mid-motion would scroll toward a cursor position that's about to
    /// change again later in the same batch, then get silently overwritten
    /// by the batch's real settle — wasted work at best, a visibly wrong
    /// intermediate scroll at worst.
    ///
    /// `viewport.scroll_row` is a DISPLAY row (what `render::build_rows`
    /// actually indexes, table borders included), but the cursor's own row
    /// is always WRAP space (border rows aren't addressable by the caret) —
    /// `view.display.wrap_to_display` converts before `reconcile` ever sees
    /// it, and the row `reconcile` hands back (also display-space) converts
    /// the OTHER way, through `display_to_wrap`, before `snap_cursor_to_row`
    /// (which computes a wrap-space cursor position) ever sees it. Missing
    /// either conversion scrolls every document containing a table wrong by
    /// the number of border rows above the cursor.
    pub fn scroll_to_cursor(&mut self, view: &ViewSnapshots) {
        if self.is_read_only() {
            if self.viewport.mode != ScrollMode::EnsureVisible {
                self.viewport.clamp_to_document(view.display.total_rows());
                return;
            }
            self.viewport.mode = ScrollMode::FollowCursor;
        }
        let display_row = self.cursor_display_row(view);
        if let Some(target_row) = self
            .viewport
            .reconcile(display_row, view.display.total_rows())
        {
            let wrap_row = view.display.display_to_wrap(target_row);
            self.snap_cursor_to_row(view, wrap_row.0);
        }
    }

    /// The DISPLAY row the primary cursor sits on, through the same
    /// buffer -> syntax -> wrap -> display chain every viewport decision
    /// needs. Both callers below depend on it being display-space: a
    /// document containing a table has border rows the wrap space knows
    /// nothing about.
    fn cursor_display_row(&self, view: &ViewSnapshots) -> DisplayRow {
        let primary = self.cursors.primary();
        let buffer_point = self.buffer.offset_to_line_col(primary.position);
        let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
        view.display.wrap_to_display(WrapRow(wrap_point.row))
    }

    /// The `Viewport::reconcile` `Independent`-mode counterpart: a
    /// `commands::nav_scroll` command already moved the viewport on its own
    /// and left the PRIMARY cursor outside the scrolloff-padded band, so it
    /// snaps onto `row` at that cursor's own `desired_col` (the same visual-
    /// column-preserving convention `commands::nav::move_row` uses) —
    /// collapsing any selection and any secondary cursor, exactly like
    /// `commands::nav::escape`'s multi-cursor collapse: the cursor is moved
    /// onto the window.
    fn snap_cursor_to_row(&mut self, view: &ViewSnapshots, row: usize) {
        let primary = self.cursors.primary();
        let col = view
            .wrap
            .byte_col_from_visual(self.buffer.content(), row, primary.desired_col);
        let syntax_point = view
            .wrap
            .wrap_to_syntax(self.buffer.content(), WrapPoint { row, col });
        let buffer_point = view.syntax.syntax_to_buffer(syntax_point);
        let offset = self.buffer.line_col_to_offset(buffer_point);
        let snapped = Cursor {
            position: offset,
            anchor: offset,
            desired_col: primary.desired_col,
            id: primary.id,
        };
        self.cursors = self.cursors.collapse_to(snapped);
    }

    /// The fixed per-BATCH settle sequence: rebuild the view, scroll to the
    /// (by now final) cursor, then reconcile the viewport a SECOND time
    /// against whatever that scroll itself settled. `App::sync_view` —
    /// called once per whole message batch by the runtime (`runtime::run`)
    /// and by tests that need the settled state — is the only caller;
    /// movement/editing commands call `view()` alone (see its docs).
    ///
    /// Re-views once more AFTER `scroll_to_cursor`, not before: reveal state
    /// is a function of `self.cursors` (`RevealGrant::Decide`'s cursor-probe
    /// policies) — a boxed table's OWN reveal state is exactly such a
    /// policy (`emit_table`: a table is rendered as its full bordered Grid/
    /// Wrapped/Pivoted layout, or (cursor inside it) as bare verbatim source
    /// lines, one whole-table `RevealSm` decision) — and `scroll_to_cursor`
    /// can itself move `self.cursors`: a `commands::nav_scroll` command's
    /// `Independent`-mode scroll leaves the viewport where it put it and
    /// instead snaps the cursor onto the now-settled window (`Viewport::
    /// reconcile`'s docs). The first `view()` above necessarily samples
    /// reveal against the PRE-scroll cursor — `scroll_to_cursor` needs that
    /// view's coordinate maps to decide where the cursor should land in the
    /// first place, so the two can't run in the other order.
    ///
    /// That first `scroll_to_cursor` call reconciles the viewport against
    /// THAT pre-final view's row geometry (`total_rows`, the wrap<->display
    /// maps) — geometry a table's own reveal transition can change out from
    /// under it: snapping the cursor INTO a boxed table collapses it from
    /// its bordered layout to bare source lines (fewer rows entirely), so
    /// the `scroll_row`/band `reconcile` just computed can land outside the
    /// scrolloff band the MOMENT the settled, reveal-updated `total_rows`
    /// replaces the stale one it was computed against — the exact "8 rows
    /// before, 9 after" `SYNC-IDEMPOTENT` shape, caught only by a later,
    /// message-free `App::sync_view` first discovering the un-reconciled
    /// mismatch. Re-viewing without reconciling again would still hand back
    /// (and let `DocMachine` cache) a snapshot whose viewport was settled
    /// against stale geometry.
    ///
    /// The second `scroll_to_cursor` call closes that gap: `mode` was
    /// already consumed back to `FollowCursor` by the first call, so this
    /// pass only ever ADJUSTS `scroll_row` to keep the (unchanged, already-
    /// settled) cursor row inside the band of the NOW-final geometry — it
    /// never moves the cursor again (`Viewport::reconcile`'s `FollowCursor`
    /// arm), so reveal cannot re-trigger from this second pass, and the
    /// final `view()` is guaranteed a `DocMachine::snapshot` memo hit. Both
    /// extra passes are free whenever nothing this batch actually triggered
    /// a reveal-driven geometry change (the common case: no scroll command,
    /// or a scroll command whose target lies outside any reveal-sensitive
    /// element).
    pub fn sync(&mut self) -> ViewSnapshots {
        let view = self.view();
        self.scroll_to_cursor(&view);
        let settled = self.view();
        self.scroll_to_cursor(&settled);
        self.view()
    }
}
