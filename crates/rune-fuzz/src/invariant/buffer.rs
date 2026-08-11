//! Buffer/line-index invariants (WP3): `BUF-LINE-INDEX`, `VERSION-
//! MONOTONE`.

use super::Violation;
use crate::snapshot::Snapshot;

/// The line index `line_starts`/`line_ends` MUST equal, derived
/// independently from `\n` byte positions in `content` — line `n` starts
/// right after the `n`th `\n` (or at 0 for the first line) and ends right
/// before the next `\n` (or at `content.len()` for the last line).
fn expected_line_bounds(content: &str) -> (Vec<usize>, Vec<usize>) {
    let mut starts = vec![0usize];
    let mut ends = Vec::new();
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            ends.push(i);
            starts.push(i + 1);
        }
    }
    ends.push(content.len());
    (starts, ends)
}

/// `BUF-LINE-INDEX` (L0) — the line index is EXACTLY the one
/// `\n` positions in `content` imply, not merely monotone/in-bounds
/// (CODE-REVIEW.md rune-fuzz finding 2: a monotone-only check lets
/// `line_starts=[0,1,2]` pass clean for `"a\nbb\nccc"`, whose real starts
/// are `[0,2,5]` — the exact off-by-one this invariant is named for).
///
/// Active-document-switch-safe: L0, a single `Snapshot`'s own fields
/// checked for internal self-consistency — there is no second document to
/// compare against.
pub fn buf_line_index(snap: &Snapshot) -> Option<Violation> {
    let (expected_starts, expected_ends) = expected_line_bounds(&snap.content);
    if snap.line_starts != expected_starts {
        return Some(Violation::new(
            "BUF-LINE-INDEX",
            format!(
                "line_starts={:?} but content's `\\n` positions imply {:?}",
                snap.line_starts, expected_starts
            ),
        ));
    }
    if snap.line_ends != expected_ends {
        return Some(Violation::new(
            "BUF-LINE-INDEX",
            format!(
                "line_ends={:?} but content's `\\n` positions imply {:?}",
                snap.line_ends, expected_ends
            ),
        ));
    }
    if snap.line_count != expected_starts.len() {
        return Some(Violation::new(
            "BUF-LINE-INDEX",
            format!(
                "line_count={} but content implies {} lines",
                snap.line_count,
                expected_starts.len()
            ),
        ));
    }
    None
}

/// `VERSION-MONOTONE` (L1) — neither `Buffer::version()` nor
/// `saved_version` ever goes backwards across a step, for the SAME
/// document. Scoped to `prev.active == next.active` for exactly the reason
/// `PANE-NO-BLEED` already is (that invariant's own docs): switching the
/// active document (e.g. `F1` toggling to/from the Help virtual document,
/// reachable since CODE-REVIEW.md rune-fuzz finding 9's fix) makes `prev`/
/// `next` describe two UNRELATED buffers, whose version numbers have no
/// ordering relationship at all — a fresh document's low version is not a
/// regression of the one it replaced as the active document.
pub fn version_monotone(prev: &Snapshot, next: &Snapshot) -> Option<Violation> {
    if prev.active != next.active {
        return None;
    }
    if next.version < prev.version {
        return Some(Violation::new(
            "VERSION-MONOTONE",
            format!("version regressed: {} -> {}", prev.version, next.version),
        ));
    }
    if next.saved_version < prev.saved_version {
        return Some(Violation::new(
            "VERSION-MONOTONE",
            format!(
                "saved_version regressed: {} -> {}",
                prev.saved_version, next.saved_version
            ),
        ));
    }
    None
}
