use rune_core::coords::DisplayRow;

// Helix's own scrolloff default.
const DEFAULT_SCROLLOFF: u16 = 5;

// FollowCursor (the default): the cursor moved and the viewport chases it,
// honouring `scrolloff`. Independent: a scroll command already moved
// `scroll_row` on its own, matching vim's "the cursor is moved onto the
// window" scrolloff behavior — the viewport stays put and `reconcile`
// snaps the cursor back into the padded band instead. `reconcile` resets
// this to `FollowCursor` once consumed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollMode {
    #[default]
    FollowCursor,
    Independent,
    EnsureVisible,
}

#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
    pub scroll_row: DisplayRow,
    pub scrolloff: u16,
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

    // A read-only document has no cursor to reconcile against; this only
    // guards the viewport against the document itself shrinking under it
    // (a resize, or a wrap-width change).
    pub fn clamp_to_document(&mut self, total_rows: usize) {
        self.scroll_row = self
            .scroll_row
            .min(DisplayRow(total_rows.saturating_sub(1)));
        self.mode = ScrollMode::FollowCursor;
    }

    // Clamped so `[scroll_row + off, scroll_row + height - 1 - off]` is
    // never empty: `(height - 1) / 2` is the largest `off` for which
    // `off <= height - 1 - off` still holds. The plainer `height / 2` would
    // let the two bounds cross on an even-height viewport, breaking the
    // one-step convergence `SYNC-IDEMPOTENT` requires.
    fn effective_scrolloff(&self) -> usize {
        let height = self.height as usize;
        (self.scrolloff as usize).min(height.saturating_sub(1) / 2)
    }

    // FollowCursor/EnsureVisible let the viewport chase the cursor inside
    // [scroll_row + off, scroll_row + height - 1 - off], returning `None`.
    // Independent leaves `scroll_row` where a scroll command already put
    // it, and instead returns the row the caller must snap the cursor to
    // if it now falls outside that band. `top`/`bottom` are clamped to the
    // document's own last row so a short document never hands back a
    // target the caller can't reach.
    pub fn reconcile(&mut self, cursor_row: DisplayRow, total_rows: usize) -> Option<DisplayRow> {
        let height = self.height as usize;
        if height == 0 {
            self.mode = ScrollMode::FollowCursor;
            return None;
        }
        let off = self.effective_scrolloff();
        let last_row = DisplayRow(total_rows.saturating_sub(1));
        self.scroll_row = self.scroll_row.min(last_row);
        let top = (self.scroll_row + off).min(last_row);
        let bottom = (self.scroll_row + height - 1 - off).min(last_row);

        match self.mode {
            ScrollMode::FollowCursor | ScrollMode::EnsureVisible => {
                self.mode = ScrollMode::FollowCursor;
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

// The one row-window slice every render-row walk shares, so a change here
// can't silently misalign parallel per-row outputs built elsewhere.
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

    const PLENTY_OF_ROWS: usize = 10_000;

    #[test]
    fn reconcile_follow_cursor_keeps_row_in_view() {
        let mut vp = viewport(80, 5);
        assert_eq!(vp.reconcile(DisplayRow(10), PLENTY_OF_ROWS), None);
        assert_eq!(vp.scroll_row, DisplayRow(6));
        assert_eq!(vp.reconcile(DisplayRow(2), PLENTY_OF_ROWS), None);
        assert_eq!(vp.scroll_row, DisplayRow(2));
    }

    #[test]
    fn reconcile_honours_scrolloff_margin() {
        let mut vp = viewport(20, 20);
        vp.scrolloff = 5;
        assert_eq!(vp.reconcile(DisplayRow(3), PLENTY_OF_ROWS), None);
        assert_eq!(vp.scroll_row, DisplayRow(0));
        assert_eq!(vp.reconcile(DisplayRow(30), PLENTY_OF_ROWS), None);
        assert_eq!(vp.scroll_row + 20 - 1 - 5, DisplayRow(30));
    }

    #[test]
    fn reconcile_converges_in_one_step() {
        let mut vp = viewport(17, 23);
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
        let mut vp = viewport(10, 10);
        vp.scrolloff = 2;
        vp.scroll_row = DisplayRow(50);
        vp.mode = ScrollMode::Independent;
        let cursor_row = DisplayRow(0);
        let snapped = vp.reconcile(cursor_row, PLENTY_OF_ROWS);
        assert_eq!(
            vp.scroll_row,
            DisplayRow(50),
            "Independent mode must not move scroll_row"
        );
        assert_eq!(snapped, Some(DisplayRow(52)));
        assert_eq!(vp.mode, ScrollMode::FollowCursor, "consumed exactly once");
    }

    #[test]
    fn reconcile_independent_mode_clamps_to_the_documents_own_last_row() {
        let mut vp = viewport(80, 20);
        vp.scrolloff = 5;
        vp.scroll_row = DisplayRow(1);
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
    fn reconcile_pulls_scroll_row_back_when_the_document_shrinks_under_it() {
        let mut vp = viewport(80, 20);
        vp.scroll_row = DisplayRow(180);
        vp.mode = ScrollMode::Independent;

        vp.reconcile(DisplayRow(0), 1);
        assert_eq!(
            vp.scroll_row,
            DisplayRow(0),
            "clamped to the shrunken document's last row"
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
