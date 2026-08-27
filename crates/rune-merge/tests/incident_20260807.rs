//! Regression gate for the 2026-08-07 real-incident corpus.
//!
//! On these bytes `ours` differs from the ancestor by exactly one
//! appended 294-byte block (the common prefix is the whole ancestor),
//! while `theirs` rewrites many interior regions; the two change sets
//! share no ancestor line, so the honest 3-way result is a clean merge
//! carrying all of theirs' edits plus ours' appended block. The old
//! rendered-diff3 parse could not see that — its alignment manufactured
//! an 8.8KB conflict, and before its anchoring fix, one whole-file
//! conflict. Position-accounted hunks localize exactly.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rune_merge::{Hunk, merge_hunks, merge_hunks_no_ancestor};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/incident-20260807/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading fixture {path}: {e}"))
}

/// The real incident bytes merge clean: one `Clean` hunk, ours' appended
/// block present verbatim, and removing that single block from the result
/// reproduces `theirs` byte-for-byte — nothing from either side is lost
/// or reordered.
#[test]
fn incident_corpus_merges_clean_with_both_sides_changes() {
    let ancestor = fixture("ancestor.md");
    let ours = fixture("ours.md");
    let theirs = fixture("theirs.md");

    let shared_prefix = ancestor
        .iter()
        .zip(ours.iter())
        .take_while(|(a, b)| a == b)
        .count();
    assert_eq!(
        shared_prefix,
        ancestor.len(),
        "ours must be the ancestor plus an appended tail"
    );
    let block = &ours[shared_prefix..];
    assert_eq!(block.len(), 294, "the incident's appended block");

    let hunks = merge_hunks(&ancestor, &ours, &theirs);

    let [Hunk::Clean(result)] = hunks.as_slice() else {
        panic!("expected one clean hunk, got {hunks:?}");
    };
    let at = result
        .windows(block.len())
        .position(|w| w == block)
        .expect("ours' appended block must survive into the result");
    let mut without_block = result[..at].to_vec();
    without_block.extend_from_slice(&result[at + block.len()..]);
    assert_eq!(
        without_block, theirs,
        "the result minus ours' block must be exactly theirs"
    );
}

/// With no known ancestor at all, `merge_hunks_no_ancestor` runs a direct
/// line diff between `ours` and `theirs` instead of feeding a synthesized
/// empty ancestor through the 3-way path (which cannot localize: diffy's
/// diff3 classifies "changed" by comparing each side against the
/// ancestor, so an empty ancestor makes the entirety of both sides count
/// as changed no matter how much they actually agree — confirmed
/// separately in the unit tests). On this corpus the two files are a near
/// full-content rewrite of each other, so the honest result is many small
/// hunks, not one — but it must still be byte-faithful and never collapse
/// to a single whole-file conflict.
#[test]
fn empty_ancestor_uses_the_2way_path_and_localizes() {
    let ours = fixture("ours.md");
    let theirs = fixture("theirs.md");

    let hunks = merge_hunks_no_ancestor(&ours, &theirs);

    assert!(
        hunks.len() > 1,
        "expected more than one hunk, got {}",
        hunks.len()
    );
    assert!(
        !hunks.iter().any(|h| matches!(
            h,
            Hunk::Conflict { ours: o, theirs: t }
                if o.len() == ours.len() && t.len() == theirs.len()
        )),
        "must not collapse to one whole-file conflict"
    );

    let mut reconstructed_ours = Vec::new();
    let mut reconstructed_theirs = Vec::new();
    for h in &hunks {
        match h {
            Hunk::Clean(b) => {
                reconstructed_ours.extend_from_slice(b);
                reconstructed_theirs.extend_from_slice(b);
            }
            Hunk::Conflict { ours, theirs } => {
                reconstructed_ours.extend_from_slice(ours);
                reconstructed_theirs.extend_from_slice(theirs);
            }
        }
    }
    assert_eq!(
        reconstructed_ours, ours,
        "hunks must reconstruct ours verbatim"
    );
    assert_eq!(
        reconstructed_theirs, theirs,
        "hunks must reconstruct theirs verbatim"
    );
}

/// Isolated regression for the exact boundary condition behind the
/// incident: a conflict section that is also the final line of an input
/// with no trailing newline. Minimal synthetic corpus, no fixture files.
#[test]
fn conflict_section_at_eof_without_trailing_newline_anchors() {
    let ancestor = b"shared\nold-tail\n";
    let ours = b"shared\nours-tail";
    let theirs = b"shared\ntheirs-tail\n";

    let hunks = merge_hunks(ancestor, ours, theirs);

    assert_eq!(
        hunks.len(),
        2,
        "expected a clean prefix and one conflict, got {hunks:?}"
    );
    assert_eq!(hunks[0], Hunk::Clean(b"shared\n".to_vec()));
    assert_eq!(
        hunks[1],
        Hunk::Conflict {
            ours: b"ours-tail".to_vec(),
            theirs: b"theirs-tail\n".to_vec(),
        }
    );
}
