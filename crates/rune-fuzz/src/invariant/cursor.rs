//! Cursor invariants (WP3): `CUR-BOUNDS`, `CUR-ORDER`, `CUR-ID`,
//! `CUR-NO-CARET-HIDDEN`, `CUR-CELL-SYNC`.

use ratatui::style::Modifier;
use rune_tui::render::Cell;

use super::{Violation, trunc};
use crate::snapshot::{Painted, Snapshot};

/// `CUR-BOUNDS` (L0) — every cursor's `position`/
/// `anchor` is a valid byte offset into `content`: in range and on a char
/// boundary.
///
/// Active-document-switch-safe: L0, checks one `Snapshot`'s cursors against
/// its own `content` — never compared to another document.
pub fn cur_bounds(snap: &Snapshot) -> Option<Violation> {
    for c in &snap.cursors {
        if c.position.get() > snap.content.len()
            || c.anchor.get() > snap.content.len()
            || !snap.content.is_char_boundary(c.position.get())
            || !snap.content.is_char_boundary(c.anchor.get())
        {
            return Some(Violation::new(
                "CUR-BOUNDS",
                format!(
                    "cursor id={} position={} anchor={} content.len()={} content={:?}",
                    c.id,
                    c.position,
                    c.anchor,
                    snap.content.len(),
                    trunc(&snap.content, 80)
                ),
            ));
        }
    }
    None
}

/// `CUR-ORDER` (L0) — cursors are ordered and non-overlapping: each
/// cursor's selection ends at or before the next cursor's selection
/// starts. Two REAL (non-collapsed) selections may legally touch (one
/// ends exactly where the next begins), so that pairing still uses `>`.
/// Two COLLAPSED cursors (bare carets, `position == anchor`) can never
/// legitimately coincide — a shared position is the canonical multi-cursor
/// defect (every edit double-applies at that byte) — so that pairing uses
/// `>=` instead (CODE-REVIEW.md rune-fuzz finding 6: `cur_id` only checks
/// id uniqueness, never position uniqueness, so two coincident collapsed
/// cursors used to pass every cursor invariant clean).
///
/// Active-document-switch-safe: L0, compares cursors within one `Snapshot`
/// only.
pub fn cur_order(snap: &Snapshot) -> Option<Violation> {
    for w in snap.cursors.windows(2) {
        if let [a, b] = w {
            let both_collapsed = a.selection_start() == a.selection_end()
                && b.selection_start() == b.selection_end();
            let violates = if both_collapsed {
                a.selection_end() >= b.selection_start()
            } else {
                a.selection_end() > b.selection_start()
            };
            if violates {
                return Some(Violation::new(
                    "CUR-ORDER",
                    format!(
                        "cursor id={} ends at {} but cursor id={} starts at {}{}",
                        a.id,
                        a.selection_end(),
                        b.id,
                        b.selection_start(),
                        if both_collapsed {
                            " (two collapsed cursors sharing the same position)"
                        } else {
                            ""
                        }
                    ),
                ));
            }
        }
    }
    None
}

/// `CUR-ID` (L0) — at least one cursor, all ids distinct. Subsumes any
/// separate cursor-count check.
///
/// Active-document-switch-safe: L0, checks one `Snapshot`'s cursor set in
/// isolation.
pub fn cur_id(snap: &Snapshot) -> Option<Violation> {
    if snap.cursors.is_empty() {
        return Some(Violation::new("CUR-ID", "cursor set is empty".to_string()));
    }
    let mut ids: Vec<_> = snap.cursors.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|w| matches!(w, [a, b] if a == b)) {
        return Some(Violation::new(
            "CUR-ID",
            format!("duplicate cursor id among {ids:?}"),
        ));
    }
    None
}

/// `CUR-NO-CARET-HIDDEN` (L0, sampled per G19) — when `caret_visible` is
/// false, no rendered cell carries `Modifier::REVERSED` outside the
/// focused reading link. Inside `Snapshot.cells` two things set that
/// modifier: `render::overlay::place_caret` and
/// `render::apply_reading_link_focus`, which paints the reading-mode link
/// highlight over `reading_link_focus` (title-bar chrome sets it too, but
/// chrome is not in the editor cell grid). A reversed cell outside that
/// focused range is therefore a caret that leaked past the gate.
///
/// Deliberately one-directional: the converse (caret visible => exactly one
/// reversed cell per cursor) is NOT asserted, because a cursor scrolled
/// above the viewport is legitimately never painted, and concealment can
/// collapse two cursors onto one cell.
///
/// Active-document-switch-safe: L0, one `Painted`'s own `cells` against
/// its own `caret_visible`.
pub fn cur_no_caret_hidden(painted: &Painted) -> Option<Violation> {
    if painted.caret_visible {
        return None;
    }
    for row in &painted.cells {
        for cell in row {
            if cell.style.add_modifier.contains(Modifier::REVERSED)
                && !reading_link_highlight(painted, cell)
            {
                return Some(Violation::new(
                    "CUR-NO-CARET-HIDDEN",
                    format!("a REVERSED cell rendered while caret_visible=false: cell={cell:?}"),
                ));
            }
        }
    }
    None
}

/// `CUR-CELL-SYNC` (L0, sampled per G19) — whenever a cursor's own logical
/// `position` is itself a byte some cell in `cells` claims (real, on-screen,
/// unconcealed), one of the REVERSED cells at that exact `buf_offset` must
/// be the caret `place_caret` painted for it: `render::segment_cells` and
/// `wrap::query::visual_col` are two separate width walks over the same
/// row, and this pins that they still agree on where the caret lands,
/// catching the class where they diverge and the caret paints onto a cell
/// whose `buf_offset` is not the cursor's own `position`.
///
/// When `position` is not claimed by any cell in `cells` at all — clamped
/// inside a concealed run (`syntax::buffer_to_syntax`'s `clamp_col`
/// redirects to `clamp_to` instead), scrolled outside the viewport, or a
/// second cursor collapsed onto the same visible cell as a first one — there
/// is no cell this cursor's own position could legitimately land on, so
/// nothing is asserted for it; the same carve-out `CUR-NO-CARET-HIDDEN`
/// already documents for a caret that is legitimately never painted.
///
/// Active-document-switch-safe: L0, one `Painted`'s own `cursors`/`cells`.
pub fn cur_cell_sync(painted: &Painted) -> Option<Violation> {
    if !painted.caret_visible {
        return None;
    }
    for cursor in &painted.cursors {
        let Ok(target) = u32::try_from(cursor.position.get()) else {
            continue;
        };
        let position_rendered = painted
            .cells
            .iter()
            .flatten()
            .any(|cell| cell.buf_offset == Some(target));
        if !position_rendered {
            continue;
        }
        let caret_painted = painted.cells.iter().flatten().any(|cell| {
            cell.buf_offset == Some(target)
                && cell.style.add_modifier.contains(Modifier::REVERSED)
                && !reading_link_highlight(painted, cell)
        });
        if !caret_painted {
            return Some(Violation::new(
                "CUR-CELL-SYNC",
                format!(
                    "cursor id={} position={} is a rendered byte but no caret-styled cell \
                     carries that buf_offset",
                    cursor.id, cursor.position
                ),
            ));
        }
    }
    None
}

fn reading_link_highlight(painted: &Painted, cell: &Cell) -> bool {
    let Some(focus) = painted.reading_link_focus else {
        return false;
    };
    let Some(offset) = cell.buf_offset else {
        return false;
    };
    focus.contains(offset as usize)
}
