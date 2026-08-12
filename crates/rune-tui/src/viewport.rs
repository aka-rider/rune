//! The scrollable window onto a document's wrapped display rows (split out
//! of `document.rs` when this package's `DocumentKind` addition
//! pushed that file past 500 lines): `Viewport` itself, its `ScrollMode`
//! (which side is authoritative for the next `reconcile` call), and the
//! vim/Helix scrolloff convergence logic.

use rune_core::coords::DisplayRow;

/// The vim/Helix scrolloff default (Helix's own default), clamped per
/// viewport at `reconcile` time (plan WP7.S1) so a tiny pane still has a
/// valid `[top, bottom]` band.
const DEFAULT_SCROLLOFF: u16 = 5;

/// Which side drives the next `Viewport::reconcile` call (plan WP7.S1):
/// `FollowCursor` — every ordinary motion command, and the default — means
/// the CURSOR moved and the viewport must chase it, honouring `scrolloff`.
/// `Independent` means a `commands::nav_scroll` scroll command already moved
/// `scroll_row` on its own (vim `scroll.txt`'s "the cursor is moved onto the
/// window" case; Helix `commands::scroll(..., sync_cursor: false)`) — the
/// viewport stays exactly where that command put it, and `reconcile` snaps
/// the CURSOR back into view instead if it fell outside the padded band.
/// `reconcile` always resets this to `FollowCursor` once consumed, so
/// exactly one `Independent` reconciliation is ever spent per scroll
/// command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollMode {
    #[default]
    FollowCursor,
    Independent,
}

/// The visible window onto the wrapped document: `width`/`height` in cells,
/// `scroll_row` the first visible DISPLAY row.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
    pub scroll_row: DisplayRow,
    /// The minimum number of wrap rows kept visible above/below the cursor
    /// (plan WP7.S1) — Helix's default (`DEFAULT_SCROLLOFF`), clamped at
    /// `reconcile` time to at most half the viewport height.
    pub scrolloff: u16,
    /// Which side is authoritative for the NEXT `reconcile` call — see
    /// `ScrollMode`'s docs. Reset to `FollowCursor` by `reconcile` itself
    /// once consumed.
    pub mode: ScrollMode,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            width: 80,
            height: 24,
            scroll_row: DisplayRow(0),
            scrolloff: DEFAULT_SCROLLOFF,
            mode: ScrollMode::FollowCursor,
        }
    }
}

impl Viewport {
    pub fn set_size(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    /// The read-only counterpart of `reconcile` (`Document::scroll_to_cursor`'s
    /// docs): a read-only document has no insertion point for the viewport to
    /// chase, so there is nothing to reconcile the cursor against — only the
    /// document itself can shrink under a scrolled-away viewport (a resize, or
    /// a wrap-width change), which this guards against. Always leaves `mode`
    /// at `FollowCursor`: idempotent by construction, since a second call with
    /// the same `total_rows` recomputes the same already-clamped `scroll_row`.
    pub fn clamp_to_document(&mut self, total_rows: usize) {
        self.scroll_row = self
            .scroll_row
            .min(DisplayRow(total_rows.saturating_sub(1)));
        self.mode = ScrollMode::FollowCursor;
    }

    /// `scrolloff`, clamped so `[scroll_row + off, scroll_row + height - 1
    /// - off]` is never empty — `(height - 1) / 2` is the largest `off` for
    /// which `off <= height - 1 - off` still holds (plan WP7.S1: "clamped
    /// to half the viewport height so it degrades in a tiny pane"). A
    /// larger clamp (plain `height / 2`) would let the two bounds cross on
    /// an even-height viewport, breaking the one-step convergence
    /// `SYNC-IDEMPOTENT` (`rune-fuzz/src/invariant/render.rs`) requires.
    fn effective_scrolloff(&self) -> usize {
        let height = self.height as usize;
        (self.scrolloff as usize).min(height.saturating_sub(1) / 2)
    }

    /// The vim/Helix scrolloff invariant (plan WP7.S1, module docs): the
    /// cursor is never left outside the viewport. Replaces the old
    /// `scroll_to_row` (vim parity note: "If the cursor position is
    /// moved off of the window, the cursor is moved onto the window (with
    /// 'scrolloff' screen lines around it)", `runtime/doc/scroll.txt`).
    ///
    /// `total_rows` is the document's own display row count — the band
    /// this clamps against, symmetrically with the existing top-of-document
    /// clamp (`reconcile_honours_scrolloff_margin`'s "can't scroll above row
    /// 0"): a document shorter than `scroll_row + height` has no row past
    /// its own last one, so neither branch may ever compute a `top`/`bottom`
    /// (or hand back a target row) beyond `total_rows - 1`. Without this, a
    /// short document pinned near its own end (`scroll_lines`'s own
    /// `max_row` clamp) could have its Independent branch hand back a row
    /// that doesn't exist; the caller (`Document::snap_cursor_to_row`,
    /// through `DisplaySnapshot::display_to_wrap`'s own clamp) would then
    /// land the cursor on the nearest REAL row instead — one still outside
    /// the band this function computed — breaking exactly the fixpoint
    /// claim below (a real regression this clamp closes: `SYNC-IDEMPOTENT`
    /// caught a scroll command on a document shorter than one scrolloff pad
    /// moving `scroll_row` again on a second, message-free reconcile).
    ///
    /// Returns `None` when the cursor's own position already satisfies the
    /// invariant (the ordinary `FollowCursor` case — the viewport moved
    /// instead) or `Some(row)` — the row the CALLER must move the cursor
    /// to — when `mode` was `Independent` and the already-settled viewport
    /// left `cursor_row` outside the padded band.
    ///
    /// Converges in exactly one call with no intervening state change
    /// (`SYNC-IDEMPOTENT`): both branches leave `cursor_row` exactly on or
    /// inside `[new_top, new_bottom]`, so calling `reconcile` again with
    /// the same `cursor_row` (and the resulting `mode == FollowCursor`)
    /// is a no-op. See the effective_scrolloff doc for why the clamp is
    /// `(height - 1) / 2`, not `height / 2`.
    pub fn reconcile(&mut self, cursor_row: DisplayRow, total_rows: usize) -> Option<DisplayRow> {
        let height = self.height as usize;
        if height == 0 {
            self.mode = ScrollMode::FollowCursor;
            return None;
        }
        let off = self.effective_scrolloff();
        let last_row = DisplayRow(total_rows.saturating_sub(1));
        let top = (self.scroll_row + off).min(last_row);
        let bottom = (self.scroll_row + height - 1 - off).min(last_row);

        match self.mode {
            ScrollMode::FollowCursor => {
                if cursor_row < top {
                    self.scroll_row = cursor_row - off;
                } else if cursor_row > bottom {
                    self.scroll_row = cursor_row + off + 1 - height;
                }
                None
            }
            ScrollMode::Independent => {
                self.mode = ScrollMode::FollowCursor;
                if cursor_row < top {
                    Some(top)
                } else if cursor_row > bottom {
                    Some(bottom)
                } else {
                    None
                }
            }
        }
    }
}

/// The ONE row-window slice both `render::build_rows` and `row_meta::
/// row_meta` walk (plan WP13.S3) — `[viewport.scroll_row, viewport.
/// scroll_row + viewport.height)` over `display`'s rows. Before this
/// chokepoint existed the two call sites each wrote the same `skip`/`take`
/// pair independently, with only a comment (not the compiler) keeping them
/// aligned; a change to one without the other would silently misalign
/// `Snapshot.cells[i]` and `Snapshot.row_meta[i]` for the session fuzzer's
/// TABLE-* invariants.
pub fn visible_rows<'a>(
    rows: &'a [rune_md::snapshot::SnapshotRow],
    viewport: &Viewport,
) -> impl Iterator<Item = &'a rune_md::snapshot::SnapshotRow> {
    rows.iter()
        .skip(viewport.scroll_row.0)
        .take(viewport.height as usize)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn viewport(width: u16, height: u16) -> Viewport {
        Viewport {
            width,
            height,
            scroll_row: DisplayRow(0),
            scrolloff: 0,
            mode: ScrollMode::FollowCursor,
        }
    }

    /// A document tall enough that none of these tests' bands ever hit the
    /// `total_rows` clamp — they exercise ordinary in-document reconciles,
    /// not the document-too-short edge case (`reconcile_independent_mode_
    /// clamps_to_the_documents_own_last_row` below covers that).
    const PLENTY_OF_ROWS: usize = 10_000;

    #[test]
    fn reconcile_follow_cursor_keeps_row_in_view() {
        // scrolloff 0 reproduces the old `scroll_to_row` behaviour exactly.
        let mut vp = viewport(80, 5);
        assert_eq!(vp.reconcile(DisplayRow(10), PLENTY_OF_ROWS), None);
        assert_eq!(vp.scroll_row, DisplayRow(6)); // 10 + 1 - 5
        assert_eq!(vp.reconcile(DisplayRow(2), PLENTY_OF_ROWS), None);
        assert_eq!(vp.scroll_row, DisplayRow(2)); // scrolled back up to keep row 2 visible
    }

    #[test]
    fn reconcile_honours_scrolloff_margin() {
        let mut vp = viewport(20, 20);
        vp.scrolloff = 5;
        // Cursor at row 3 must be at least 5 rows from the top.
        assert_eq!(vp.reconcile(DisplayRow(3), PLENTY_OF_ROWS), None);
        assert_eq!(vp.scroll_row, DisplayRow(0)); // clamped: can't scroll above row 0
        assert_eq!(vp.reconcile(DisplayRow(30), PLENTY_OF_ROWS), None);
        // top = scroll_row + 5, bottom = scroll_row + 20 - 1 - 5: row 30 must
        // land exactly on the bottom margin.
        assert_eq!(vp.scroll_row + 20 - 1 - 5, DisplayRow(30));
    }

    #[test]
    fn reconcile_converges_in_one_step() {
        // `SYNC-IDEMPOTENT` (rune-fuzz/src/invariant/render.rs): a second
        // `reconcile` call with the SAME cursor row must never move
        // `scroll_row` again.
        let mut vp = viewport(17, 23); // odd dimensions exercise the clamp
        vp.scrolloff = 5;
        for cursor_row in [0usize, 3, 11, 47, 199].map(DisplayRow) {
            vp.reconcile(cursor_row, PLENTY_OF_ROWS);
            let scroll_before = vp.scroll_row;
            assert_eq!(
                vp.reconcile(cursor_row, PLENTY_OF_ROWS),
                None,
                "must not need a cursor snap"
            );
            assert_eq!(
                vp.scroll_row, scroll_before,
                "a second reconcile with the same cursor row moved scroll_row"
            );
        }
    }

    #[test]
    fn reconcile_independent_mode_snaps_the_cursor_never_the_viewport() {
        // A `commands::nav_scroll` command already moved `scroll_row` and
        // armed `Independent` mode; the viewport scrolled far enough away
        // that the (unmoved) cursor now sits outside the padded band —
        // `reconcile` must return the boundary row to snap the CURSOR to,
        // and must NOT move `scroll_row` itself (plan WP7.S1: "the cursor
        // is moved onto the window", not the other way around).
        let mut vp = viewport(10, 10);
        vp.scrolloff = 2;
        vp.scroll_row = DisplayRow(50);
        vp.mode = ScrollMode::Independent;
        let cursor_row = DisplayRow(0); // far above the new viewport
        let snapped = vp.reconcile(cursor_row, PLENTY_OF_ROWS);
        assert_eq!(
            vp.scroll_row,
            DisplayRow(50),
            "Independent mode must not move scroll_row"
        );
        assert_eq!(snapped, Some(DisplayRow(52))); // top = scroll_row(50) + off(2)
        assert_eq!(vp.mode, ScrollMode::FollowCursor, "consumed exactly once");
    }

    /// The regression this clamp fixes (`TODO-sync-idempotent-link-reveal-
    /// lag.md`'s second repro): a document with only 2 rows total, scrolled
    /// (`Independent` mode) to its own last row via `scroll_lines`'s own
    /// `max_row` clamp. The unclamped formula would compute a band
    /// (`top`/`bottom`) past row 1 and hand back a row that doesn't exist
    /// — the caller can only land the cursor on a REAL row (`display_to_
    /// wrap`'s own clamp), leaving it still outside that impossible band,
    /// so a later `FollowCursor` reconcile with the identical cursor row
    /// would move `scroll_row` again. `total_rows` must confine both `top`
    /// and `bottom` to `[0, total_rows - 1]`, exactly like the existing
    /// top-of-document clamp already confines them to `>= 0`, so the
    /// target this hands back is always one the caller can actually reach.
    #[test]
    fn reconcile_independent_mode_clamps_to_the_documents_own_last_row() {
        let mut vp = viewport(80, 20);
        vp.scrolloff = 5;
        vp.scroll_row = DisplayRow(1); // pinned at the last row of a 2-row document
        vp.mode = ScrollMode::Independent;
        let total_rows = 2;

        let snapped = vp.reconcile(DisplayRow(0), total_rows);
        assert_eq!(
            snapped,
            Some(DisplayRow(1)),
            "must hand back the document's own last row, never a row past it"
        );
        assert_eq!(
            vp.scroll_row,
            DisplayRow(1),
            "Independent mode must not move scroll_row"
        );

        // The settled state must now be a genuine fixpoint: a second,
        // message-free reconcile with the cursor on the row it was just
        // snapped to must not move `scroll_row` again.
        let scroll_before = vp.scroll_row;
        assert_eq!(
            vp.reconcile(DisplayRow(1), total_rows),
            None,
            "the settled cursor row must already satisfy the (clamped) band"
        );
        assert_eq!(
            vp.scroll_row, scroll_before,
            "a second reconcile with the settled cursor row moved scroll_row"
        );
    }

    #[test]
    fn clamp_to_document_is_idempotent_and_resets_to_follow_cursor() {
        let mut vp = viewport(80, 20);
        vp.scroll_row = DisplayRow(90);
        vp.mode = ScrollMode::Independent;

        vp.clamp_to_document(100);
        assert_eq!(
            vp.scroll_row,
            DisplayRow(90),
            "still within the document, unchanged"
        );
        assert_eq!(vp.mode, ScrollMode::FollowCursor);

        let scroll_before = vp.scroll_row;
        vp.clamp_to_document(100);
        assert_eq!(
            vp.scroll_row, scroll_before,
            "a second call must not move it again"
        );
    }

    #[test]
    fn clamp_to_document_pulls_scroll_row_back_when_the_document_shrinks() {
        let mut vp = viewport(80, 20);
        vp.scroll_row = DisplayRow(90);
        vp.mode = ScrollMode::Independent;

        vp.clamp_to_document(10);
        assert_eq!(
            vp.scroll_row,
            DisplayRow(9),
            "clamped to the shorter document's last row"
        );
        assert_eq!(vp.mode, ScrollMode::FollowCursor);
    }
}
