//! The `Cell` rows -> `ratatui::Frame` blit (split out of `render` per
//! §1.6) — the single writer of terminal buffer cells from a rendered
//! `Vec<Vec<Cell>>`.

use ratatui::Frame;
use ratatui::buffer::CellWidth;
use ratatui::layout::Rect;

use super::Cell;

/// Mirrors `rune-syntax`'s and `rune-md`'s own identically-named
/// `STRICT_INVARIANTS`/`assert_invariant` chokepoint: `true` only in test
/// builds or when this crate's own `strict-invariants` feature is
/// explicitly enabled. Kept as a local copy rather than a shared helper —
/// each crate's gate governs only its own producer-bug invariants.
const STRICT_INVARIANTS: bool = cfg!(any(test, feature = "strict-invariants"));

/// The chokepoint every "this should never happen, but let's be sure"
/// blit-layer check in this module routes through — CONSTITUTION §1.3
/// forbids `panic!`/`assert!`/`unwrap` in production code paths, so an
/// ordinary build (including a plain `cargo run`) must degrade gracefully
/// on an invariant violation rather than take down the user's session;
/// only a test run or an explicit opt-in feature treats it as fatal.
///
/// `cond` is a closure rather than a `bool` because this module's checks sit
/// in the per-cell render hot path: re-deriving a symbol's width to compare
/// against it costs a Unicode table walk per cell per frame, and an eagerly
/// evaluated argument would pay that cost in ordinary builds that then throw
/// the answer away.
fn assert_invariant(cond: impl FnOnce() -> bool, msg: impl FnOnce() -> String) {
    if STRICT_INVARIANTS {
        assert!(cond(), "{}", msg());
    }
}

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
/// chokepoint's own invariant, and the `assert_invariant` call below
/// enforces it at every cell this loop writes; a producer bug that lets the
/// two drift apart again would corrupt the row exactly as the pre-fix
/// MAX-rule divergence once did, so this is deliberately checked here too,
/// not just at the width chokepoint itself.
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
            assert_invariant(
                || usize::from(cell.width) == cell.text.cell_width() as usize,
                || {
                    format!(
                        "Cell width {} disagrees with ratatui's own cell_width() {} for symbol {:?}",
                        cell.width,
                        cell.text.cell_width(),
                        cell.text
                    )
                },
            );
            let width = u16::from(cell.width.max(1));
            // WP13.S2: a cell that *starts* inside `area` can still not
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
