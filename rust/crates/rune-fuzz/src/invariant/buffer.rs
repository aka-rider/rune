//! Buffer/line-index invariants (WP3): `BUF-LINE-INDEX`, `VERSION-
//! MONOTONE`.

use super::Violation;
use crate::snapshot::Snapshot;

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
