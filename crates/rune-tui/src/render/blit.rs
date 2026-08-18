//! The `Cell` rows -> `ratatui::Frame` blit (500-line budget split out of
//! `render`) — the single writer of terminal buffer cells from a rendered
//! `Vec<Vec<Cell>>`.

use ratatui::Frame;
use ratatui::buffer::CellWidth;
use ratatui::layout::Rect;
use rune_core::assert_invariant;

use super::Cell;

/// Writes `rows` into `frame.buffer_mut()` starting at `area`'s top-left
/// corner, clipping to `area`'s bounds.
///
/// A `Cell` wider than 1 column (a wide CJK char, or a multi-codepoint
/// grapheme cluster like a ZWJ emoji sequence — `push_grapheme_cells`'s
/// docs) needs its OWN width's worth of buffer columns explicitly reset
/// (`ratatui::buffer::Cell::reset`), not just skipped over: `cell_mut`
/// writes one column at a time and never touches its neighbors, unlike
/// `Buffer::set_stringn` (which this code deliberately doesn't use — it
/// only ever writes ONE known-width symbol at a time, not a whole string to
/// re-measure), so without this loop the "continuation" column(s) a wide
/// cell covers keep whatever a PRIOR frame happened to leave there.
/// Ratatui's own diffing (`BufferDiff`, ratatui-core) silently skips
/// re-examining exactly that many columns after any cell whose OWN
/// `cell_width()` is `> 1` — "we're assuming buffers are well-formed, that
/// is no double-width cell is followed by a non-blank cell" — so leftover,
/// non-blank content there would never even reach the real terminal's
/// diff/redraw, breaking the ZWJ fix at the last step. Resetting by THIS
/// `Cell`'s own `width` depends on that width actually AGREEING with what
/// ratatui's own `CellWidth for str` derives for the same symbol —
/// `rune_syntax::wrap::grapheme_width`'s doc comment states that as the
/// chokepoint's own invariant — and the `assert_invariant` call below
/// enforces exactly that (with one narrow exception, that assert's own doc)
/// at every cell this loop writes; a producer bug that lets the two drift
/// apart in any OTHER way would corrupt the row exactly as the pre-fix
/// MAX-rule divergence once did, so this is deliberately checked here too,
/// not just at the width chokepoint itself.
///
/// A `Cell` at width 0 (a lone zero-width rune, `grapheme_width`'s doc)
/// still gets written here — `target.set_symbol` — but `x` never advances
/// past it, so whatever `Cell` comes next on the row writes into the SAME
/// buffer column, overwriting it. That is the correct outcome, not a bug: a
/// zero-width rune has no column of its own on a real terminal either, so
/// the glyph a reader actually sees there is whichever `Cell` a real
/// terminal would ALSO land on that column — the one right after it, if any
/// — never the invisible rune itself. `render::overlay::place_caret`
/// mirrors this same column-collapse when painting the caret, so the two
/// stay in agreement about which `Cell` a shared column's REVERSED style
/// ends up surviving on.
pub fn blit(rows: &[Vec<Cell>], area: Rect, frame: &mut Frame) {
    let buf = frame.buffer_mut();
    let right = area.x.saturating_add(area.width);
    for (row_idx, row) in rows.iter().enumerate() {
        let y = area.y.saturating_add(row_idx as u16);
        if y >= area.y.saturating_add(area.height) {
            break;
        }
        let mut x = area.x;
        for cell in row {
            if x >= right {
                break;
            }
            assert_invariant!(
                {
                    let declared = usize::from(cell.width);
                    let ratatui_width = cell.text.cell_width() as usize;
                    declared == ratatui_width
                        // The ONE remaining narrow exception, scoped to
                        // MULTI-rune clusters only: `grapheme_width`'s own
                        // multi-rune floor (never zero, `CELL-OFFSET`'s
                        // invariant) can still disagree with ratatui for a
                        // pathological all-zero-width multi-char cluster —
                        // a single-`char` lone rune no longer needs this,
                        // since `control_aware_width` now returns ratatui's
                        // own 0 for it directly (that case hits the plain
                        // `declared == ratatui_width` branch above).
                        || (declared == 1
                            && ratatui_width == 0
                            && cell.text.chars().count() > 1)
                },
                || {
                    format!(
                        "Cell width {} disagrees with ratatui's own cell_width() {} for symbol {:?}",
                        cell.width,
                        cell.text.cell_width(),
                        cell.text
                    )
                },
            );
            let width = u16::from(cell.width);
            // A cell that *starts* inside `area` can still not
            // *fit* — a double-width glyph landing on the last column
            // would need a continuation cell past `right` that this loop
            // never writes, leaving the border's own cell un-reset there
            // (ratatui's diffing then never revisits it, so the gap
            // persists across frames — the resize-race defect this guards
            // against). Substitute a single blank cell instead of the
            // glyph whenever it wouldn't fully fit.
            let fits = x.saturating_add(width) <= right;
            if let Some(target) = buf.cell_mut((x, y)) {
                if fits {
                    target.set_symbol(&cell.text);
                } else {
                    target.set_symbol(" ");
                }
                target.set_style(cell.style);
            }
            if fits {
                for dx in 1..width {
                    let cx = x.saturating_add(dx);
                    if cx >= right {
                        break;
                    }
                    if let Some(cont) = buf.cell_mut((cx, y)) {
                        cont.reset();
                    }
                }
            }
            x = x.saturating_add(width);
        }
    }
}
