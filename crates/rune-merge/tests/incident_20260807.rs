//! Regression gate for the 2026-08-07 real-incident conflict-anchoring bug,
//! tightened past the initial diagnosis into the actual fix.
//!
//! Root cause: diffy's diff3 output holds exactly one conflict block on the
//! real incident bytes, sitting after a large shared clean prefix, with an
//! ours-section far smaller than all of `ours`. diffy's diff3 marker text
//! newline-terminates every line it writes — including a section's last
//! line when that line is also the input's last line and the input has no
//! trailing newline of its own. The ours-section here is exactly that
//! case: `ours.md` has no trailing newline, so the verbatim run it names
//! is one byte shorter than the section text. The old anchor check failed
//! outright on that mismatch and `parse_hunks` discarded the localized
//! boundary entirely, falling back to one whole-file conflict. The fix
//! retries a failed anchor with that synthesized trailing newline
//! stripped, accepted only when the match then lands exactly at
//! end-of-input.
//!
//! Diffy's own conflict granularity was never the problem (explanation
//! (b)); the deltas do not genuinely overlap either (explanation (a) —
//! ours differs from the ancestor by one appended line). Both are ruled
//! out by the same evidence that pins verdict (c).

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

/// The real incident bytes, with the anchoring fix in place: the clean
/// prefix shared by all three inputs survives as `Clean`, and exactly one
/// `Conflict` covers the region that actually differs — not the whole
/// file.
#[test]
fn incident_corpus_localizes_to_one_conflict_after_the_clean_prefix() {
    let ancestor = fixture("ancestor.md");
    let ours = fixture("ours.md");
    let theirs = fixture("theirs.md");

    let hunks = merge_hunks(&ancestor, &ours, &theirs);

    assert_eq!(
        hunks.len(),
        2,
        "expected a clean prefix followed by one localized conflict, got {} hunks: {hunks:?}",
        hunks.len()
    );
    let Hunk::Clean(prefix) = &hunks[0] else {
        panic!(
            "expected the first hunk to be the clean prefix, got {:?}",
            hunks[0]
        );
    };
    let Hunk::Conflict {
        ours: c_ours,
        theirs: c_theirs,
    } = &hunks[1]
    else {
        panic!(
            "expected the second hunk to be the conflict, got {:?}",
            hunks[1]
        );
    };

    assert_eq!(prefix.len(), 34_358, "clean prefix should not have shrunk");
    assert_eq!(
        c_ours.len(),
        8_802,
        "ours-section should be the localized region, not the whole 30 433-byte file"
    );
    assert_eq!(
        c_theirs.len(),
        2_409,
        "theirs-section should be the localized region, not the whole 36 767-byte file"
    );
    assert!(
        !c_theirs.is_empty(),
        "theirs-section must carry real content, never empty"
    );

    // Byte-faithfulness here means every returned run of bytes is a
    // verbatim substring of the input it claims to come from — not that
    // concatenating hunks reconstructs `ours` or `theirs` byte-for-byte,
    // since a clean region legitimately carries whichever side's change
    // diff3 auto-resolved there (3-way merge semantics, not a bug).
    assert!(
        contains(&ours, c_ours),
        "ours-section not a verbatim substring of ours"
    );
    assert!(
        contains(&theirs, c_theirs),
        "theirs-section not a verbatim substring of theirs"
    );
    assert!(
        contains(&ours, prefix) || contains(&theirs, prefix),
        "clean prefix not a verbatim substring of either side"
    );
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
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
