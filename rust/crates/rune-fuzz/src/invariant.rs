//! Named invariant checkers over `Snapshot`/`StepCtx`. Mirrors `internal/
//! fuzz/invariant/invariant.go`'s `Violation` + `Trunc`, and the per-domain
//! checker style of `internal/fuzz/ui/textedit/textedit.go` /
//! `internal/fuzz/editor/display/display_invariant_test.go`.
//!
//! Three checker shapes, all pure and all over owned data, so every one is
//! independently unit-testable (plan Risk R-c):
//! - L0: `fn(&Snapshot) -> Option<Violation>` — a single-state property.
//! - L1: `fn(&Snapshot, &Snapshot) -> Option<Violation>` — a transition.
//! - L2: `fn(&Snapshot, &Snapshot, &StepCtx) -> Option<Violation>` — a
//!   transition that also needs what message caused it. WP3 ships none of
//!   these (its six invariants are L0/L1 only); `check_all` already
//!   receives `StepCtx` so a later work package's L2 checkers slot in
//!   without changing this function's signature.
//!
//! WP3 ships six invariants, evaluated first-wins in the order below.
//! `NO-PANIC` is not a checker function here — the driver constructs it
//! directly from a caught unwind (`driver.rs`).

use crate::snapshot::Snapshot;
use crate::step::StepCtx;

/// A failed invariant check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub id: &'static str,
    pub message: String,
}

/// Truncating formatter for message payloads — Go analogue:
/// `invariant.Trunc`. Never slices mid-character (`clippy::indexing_
/// slicing` is denied under `-D warnings`; `str::get` returns `None`
/// instead of panicking on a bad range).
pub fn trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    match s.get(..end) {
        Some(head) => format!("{head}…"),
        None => s.to_string(),
    }
}

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

/// `CUR-ORDER` (L0, Go `C1` at `internal/fuzz/ui/textedit/textedit.go:254-
/// 267`) — cursors are ordered and non-overlapping: each cursor's
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

/// `BUF-LINE-INDEX` (L0, Go `B1`) — the line index is internally
/// consistent: `line_count` matches the number of `\n`-delimited lines,
/// `line_starts` begins at 0 and strictly increases, and every `line_
/// start`/`line_end` is in bounds and on a char boundary.
pub fn buf_line_index(snap: &Snapshot) -> Option<Violation> {
    let expected_line_count = snap.content.matches('\n').count() + 1;
    if snap.line_count != expected_line_count {
        return Some(Violation {
            id: "BUF-LINE-INDEX",
            message: format!(
                "line_count={} but content implies {expected_line_count} lines",
                snap.line_count
            ),
        });
    }
    if snap.line_starts.first().copied() != Some(0) {
        return Some(Violation {
            id: "BUF-LINE-INDEX",
            message: format!(
                "line_starts[0]={:?}, want Some(0)",
                snap.line_starts.first()
            ),
        });
    }
    for w in snap.line_starts.windows(2) {
        if let [a, b] = w
            && b <= a
        {
            return Some(Violation {
                id: "BUF-LINE-INDEX",
                message: format!("line_starts not strictly increasing: {a} then {b}"),
            });
        }
    }
    for (n, (&start, &end)) in snap
        .line_starts
        .iter()
        .zip(snap.line_ends.iter())
        .enumerate()
    {
        let in_bounds = start <= snap.content.len() && end <= snap.content.len();
        let on_boundary =
            snap.content.is_char_boundary(start) && snap.content.is_char_boundary(end);
        if !in_bounds || !on_boundary || end < start {
            return Some(Violation {
                id: "BUF-LINE-INDEX",
                message: format!(
                    "line {n}: start={start} end={end} content.len()={} in_bounds={in_bounds} \
                     on_boundary={on_boundary}",
                    snap.content.len()
                ),
            });
        }
    }
    None
}

/// `VERSION-MONOTONE` (L1, Go `B2`) — neither `Buffer::version()` nor
/// `saved_version` ever goes backwards across a step.
pub fn version_monotone(prev: &Snapshot, next: &Snapshot) -> Option<Violation> {
    if next.version < prev.version {
        return Some(Violation {
            id: "VERSION-MONOTONE",
            message: format!("version regressed: {} -> {}", prev.version, next.version),
        });
    }
    if next.saved_version < prev.saved_version {
        return Some(Violation {
            id: "VERSION-MONOTONE",
            message: format!(
                "saved_version regressed: {} -> {}",
                prev.saved_version, next.saved_version
            ),
        });
    }
    None
}

/// Runs every WP3 checker, first-wins, in the order fixed by WP3.S6.
/// `_ctx` is unused by any WP3 checker — reserved for the L2 shape a later
/// work package adds — so `check_all`'s signature never has to change.
pub fn check_all(prev: &Snapshot, next: &Snapshot, _ctx: &StepCtx) -> Option<Violation> {
    cur_bounds(next)
        .or_else(|| cur_order(next))
        .or_else(|| cur_id(next))
        .or_else(|| buf_line_index(next))
        .or_else(|| version_monotone(prev, next))
}
