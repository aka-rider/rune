//! Cursor invariants (WP3): `CUR-BOUNDS`, `CUR-ORDER`, `CUR-ID`.

use super::{Violation, trunc};
use crate::snapshot::Snapshot;

/// `CUR-BOUNDS` (L0, §1.3 clamp / §1.5 bytes) — every cursor's `position`/
/// `anchor` is a valid byte offset into `content`: in range and on a char
/// boundary.
pub fn cur_bounds(snap: &Snapshot) -> Option<Violation> {
    for c in &snap.cursors {
        if c.position > snap.content.len()
            || c.anchor > snap.content.len()
            || !snap.content.is_char_boundary(c.position)
            || !snap.content.is_char_boundary(c.anchor)
        {
            return Some(Violation {
                id: "CUR-BOUNDS",
                message: format!(
                    "cursor id={} position={} anchor={} content.len()={} content={:?}",
                    c.id,
                    c.position,
                    c.anchor,
                    snap.content.len(),
                    trunc(&snap.content, 80)
                ),
            });
        }
    }
    None
}

/// `CUR-ORDER` (L0, Go `C1`) — cursors are ordered and non-overlapping: each cursor's
/// selection ends at or before the next cursor's selection starts.
pub fn cur_order(snap: &Snapshot) -> Option<Violation> {
    for w in snap.cursors.windows(2) {
        if let [a, b] = w
            && a.selection_end() > b.selection_start()
        {
            return Some(Violation {
                id: "CUR-ORDER",
                message: format!(
                    "cursor id={} ends at {} but cursor id={} starts at {}",
                    a.id,
                    a.selection_end(),
                    b.id,
                    b.selection_start()
                ),
            });
        }
    }
    None
}

/// `CUR-ID` (L0, Go `C2` at `textedit.go:269-287`) — at least one cursor,
/// every id non-zero, all ids distinct. Subsumes any separate cursor-count
/// check.
pub fn cur_id(snap: &Snapshot) -> Option<Violation> {
    if snap.cursors.is_empty() {
        return Some(Violation {
            id: "CUR-ID",
            message: "cursor set is empty".to_string(),
        });
    }
    for c in &snap.cursors {
        if c.id == 0 {
            return Some(Violation {
                id: "CUR-ID",
                message: format!("cursor with id=0 at position={}", c.position),
            });
        }
    }
    let mut ids: Vec<u32> = snap.cursors.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|w| matches!(w, [a, b] if a == b)) {
        return Some(Violation {
            id: "CUR-ID",
            message: format!("duplicate cursor id among {ids:?}"),
        });
    }
    None
}
