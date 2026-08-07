//! WP1 diagnosis gate for the 2026-08-07 `CONSTITUTION.md` incident (plan
//! `bug-i-edited-quirky-turtle.md`): replays the real incident bytes
//! through `merge_hunks` to settle which of three explanations produced
//! the terrible merge the user saw.
//!
//! Verdict: (c), rune-merge's own anchoring degradation. Traced with
//! `diffy::MergeOptions::merge_bytes` directly: diffy's diff3 output holds
//! exactly one conflict block, and that block is *not* whole-file — it
//! sits after a large shared clean prefix, with an ours-section far
//! smaller than all of `ours`. The ours-section text is one byte longer
//! than the verbatim run it names in `ours` (a trailing-newline mismatch
//! at its end-of-file boundary), so re-anchoring it fails, `parse_hunks`
//! discards the localized boundary, and the whole-file fallback returns
//! `ours`/`theirs` in full. Diffy's own conflict granularity (explanation
//! (b)) was never the problem here; the deltas do not genuinely overlap
//! (explanation (a) is also ruled out — ours differs from the ancestor by
//! one appended line).
//!
//! The tests below pin today's degenerate output for this corpus as a
//! characterization test. They must keep passing unchanged until the
//! merge remedy work (WP-D) fixes the anchoring fallback or adds sub-hunk
//! resolution; at that point these assertions are expected to need
//! tightening, not the fixtures.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_merge::{Hunk, merge_hunks};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/incident-20260807/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading fixture {path}: {e}"))
}

/// Pins the incident corpus's current behavior: one whole-file `Conflict`
/// hunk, `ours` and `theirs` each returned in full rather than as the
/// localized region diffy actually identified. This is the anchoring
/// degradation (verdict c), not genuine overlap (a) or diffy's own
/// conflict granularity (b).
#[test]
fn incident_corpus_degrades_to_one_whole_file_conflict() {
    let ancestor = fixture("ancestor.md");
    let ours = fixture("ours.md");
    let theirs = fixture("theirs.md");

    let hunks = merge_hunks(&ancestor, &ours, &theirs);

    assert_eq!(
        hunks.len(),
        1,
        "expected the anchoring fallback's single whole-file hunk, got {} hunks",
        hunks.len()
    );
    let Hunk::Conflict {
        ours: c_ours,
        theirs: c_theirs,
    } = &hunks[0]
    else {
        panic!("expected a Conflict hunk, got {:?}", hunks[0]);
    };
    assert_eq!(c_ours, &ours, "ours side is not the whole ours input");
    assert_eq!(
        c_theirs, &theirs,
        "theirs side is not the whole theirs input"
    );
}

/// Pins the empty-ancestor path (`landing.rs`'s
/// `ancestor_text.as_deref().unwrap_or("")` degrades a missing ancestor to
/// an empty one) against the same corpus: it produces the identical
/// whole-file-conflict shape as a real ancestor does here, not an empty or
/// panicking result.
#[test]
fn empty_ancestor_also_yields_one_whole_file_conflict() {
    let ours = fixture("ours.md");
    let theirs = fixture("theirs.md");

    let hunks = merge_hunks(b"", &ours, &theirs);

    assert_eq!(hunks.len(), 1, "expected one hunk, got {}", hunks.len());
    let Hunk::Conflict {
        ours: c_ours,
        theirs: c_theirs,
    } = &hunks[0]
    else {
        panic!("expected a Conflict hunk, got {:?}", hunks[0]);
    };
    assert_eq!(c_ours, &ours, "ours side is not the whole ours input");
    assert_eq!(
        c_theirs, &theirs,
        "theirs side is not the whole theirs input"
    );
}
