//! Cursor invariants (WP3): `CUR-BOUNDS`, `CUR-ORDER`, `CUR-ID`,
//! `CUR-NO-CARET-HIDDEN`.

use ratatui::style::Modifier;

use super::{Violation, trunc};
use crate::snapshot::Snapshot;

/// `CUR-BOUNDS` (L0) — every cursor's `position`/
/// `anchor` is a valid byte offset into `content`: in range and on a char
/// boundary.
///
/// Active-document-switch-safe: L0, checks one `Snapshot`'s cursors against
/// its own `content` — never compared to another document.
pub fn cur_bounds(snap: &Snapshot) -> Option<Violation> {
    for c in &snap.cursors {
        if c.position > snap.content.len()
            || c.anchor > snap.content.len()
            || !snap.content.is_char_boundary(c.position)
            || !snap.content.is_char_boundary(c.anchor)
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
/// false, NO rendered cell carries `Modifier::REVERSED`. Inside
/// `Snapshot.cells` that modifier is set by exactly one thing,
/// `render::overlay::place_caret` (title-bar chrome sets it too, but chrome
/// is not in the editor cell grid), so a reversed cell in a hidden-caret
/// snapshot is a caret that leaked past the gate.
///
/// Deliberately one-directional: the converse (caret visible => exactly one
/// reversed cell per cursor) is NOT asserted, because a cursor scrolled
/// above the viewport is legitimately never painted, and concealment can
/// collapse two cursors onto one cell.
///
/// Active-document-switch-safe: L0, one `Snapshot`'s own `cells` against
/// its own `caret_visible`.
pub fn cur_no_caret_hidden(snap: &Snapshot) -> Option<Violation> {
    if snap.caret_visible {
        return None;
    }
    for row in &snap.cells {
        for cell in row {
            if cell.style.add_modifier.contains(Modifier::REVERSED) {
                return Some(Violation::new(
                    "CUR-NO-CARET-HIDDEN",
                    format!("a REVERSED cell rendered while caret_visible=false: cell={cell:?}"),
                ));
            }
        }
    }
    None
}
