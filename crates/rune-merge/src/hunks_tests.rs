#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]

use super::*;

fn round_trip(hunks: &[Hunk]) -> Vec<u8> {
    let mut buf = Vec::new();
    for h in hunks {
        match h {
            Hunk::Clean(bytes) => buf.extend_from_slice(bytes),
            Hunk::Conflict { ours, .. } => buf.extend_from_slice(ours),
        }
    }
    buf
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find_subslice(haystack, needle).is_some()
}

// Separate changed lines with a shared line so diffy's Myers diff does
// not treat them as adjacent (conflicting) hunks.
#[test]
fn clean_merge_has_no_conflicts() {
    let ancestor = b"line1\nshared-A\nline2\nshared-B\nline3\n";
    let ours = b"line1\nchanged-by-ours\nline2\nshared-B\nline3\n";
    let theirs = b"line1\nshared-A\nline2\nchanged-by-theirs\nline3\n";

    let hunks = merge_hunks(ancestor, ours, theirs);
    assert!(!hunks.is_empty());
    for h in &hunks {
        assert!(
            !matches!(h, Hunk::Conflict { .. }),
            "expected no conflicts for non-overlapping changes: {h:?}"
        );
    }
}

#[test]
fn overlapping_changes_conflict_verbatim() {
    let ancestor = b"line1\nshared\nline3\n";
    let ours = b"line1\nours-changed\nline3\n";
    let theirs = b"line1\ntheirs-changed\nline3\n";

    let hunks = merge_hunks(ancestor, ours, theirs);
    let conflict = hunks
        .iter()
        .find(|h| matches!(h, Hunk::Conflict { .. }))
        .expect("expected at least one conflict");
    let Hunk::Conflict {
        ours: c_ours,
        theirs: c_theirs,
    } = conflict
    else {
        unreachable!("filtered above")
    };
    assert!(contains(ours, c_ours), "ours bytes not verbatim");
    assert!(contains(theirs, c_theirs), "theirs bytes not verbatim");
}

#[test]
fn marker_shaped_document_content_segments_correctly() {
    let marker_block = "<<<<<<<\n|||||||\n=======\n>>>>>>>\n";
    let ancestor = format!("{marker_block}before\nshared\nafter\n");
    let ours = format!("{marker_block}before\nours-change\nafter\n");
    let theirs = format!("{marker_block}before\ntheirs-change\nafter\n");

    let hunks = merge_hunks(ancestor.as_bytes(), ours.as_bytes(), theirs.as_bytes());

    assert_eq!(
        hunks,
        vec![
            Hunk::Clean(format!("{marker_block}before\n").into_bytes()),
            Hunk::Conflict {
                ours: b"ours-change\n".to_vec(),
                theirs: b"theirs-change\n".to_vec(),
            },
            Hunk::Clean(b"after\n".to_vec()),
        ]
    );
}

#[test]
fn conflict_marker_length_picks_max_run_across_all_inputs() {
    let ancestor = b"|||||||||\nrest\n";
    let ours = b"hello\n";
    let theirs = b"<<<<<<<<\nworld\n";
    assert_eq!(conflict_marker_length(ancestor, ours, theirs), 10);
}

#[test]
fn conflict_marker_length_defaults_to_seven_without_marker_like_content() {
    let ancestor = b"hello\n";
    let ours = b"world\n";
    let theirs = b"there\n";
    assert_eq!(conflict_marker_length(ancestor, ours, theirs), 7);
}

#[test]
fn crlf_preserved_in_ours() {
    let ancestor = b"line1\r\nancestor\r\nline3\r\n";
    let ours = b"line1\r\nours-changed\r\nline3\r\n";
    let theirs = b"line1\r\nancestor\r\nline3-theirs\r\n";

    let merged = round_trip(&merge_hunks(ancestor, ours, theirs));
    assert!(contains(&merged, b"\r\n"), "CRLF normalized away");
    assert!(
        contains(&merged, b"ours-changed\r\n"),
        "ours-changed CRLF not preserved"
    );
}

#[test]
fn no_trailing_newline_is_not_added() {
    let ancestor = b"hello";
    let ours = b"hello-ours";
    let theirs = b"hello";

    let merged = round_trip(&merge_hunks(ancestor, ours, theirs));
    assert_ne!(merged.last(), Some(&b'\n'), "trailing newline added");
}

#[test]
fn bom_preserved() {
    const BOM: &[u8] = b"\xef\xbb\xbf";
    let ancestor = [BOM, b"hello\n"].concat();
    let ours = [BOM, b"hello-ours\n"].concat();
    let theirs = [BOM, b"hello\n"].concat();

    let merged = round_trip(&merge_hunks(&ancestor, &ours, &theirs));
    assert!(merged.starts_with(BOM), "BOM stripped");
}

#[test]
fn clean_theirs_change_is_verbatim() {
    let ancestor = b"line1\noriginal\nline3\n";
    let ours = b"line1\noriginal\nline3\n";
    let theirs = b"line1\ntheirs-version\nline3\n";

    let hunks = merge_hunks(ancestor, ours, theirs);
    let merged = round_trip(&hunks);
    assert!(contains(&merged, b"theirs-version\n"));
    for h in &hunks {
        if let Hunk::Clean(bytes) = h
            && contains(bytes, b"theirs-version")
        {
            assert!(contains(theirs, bytes), "not verbatim from theirs");
        }
    }
}

#[test]
fn ours_only_change_is_clean() {
    let ancestor = b"line1\nshared\nline3\n";
    let ours = b"line1\nours-only\nline3\n";
    let theirs = b"line1\nshared\nline3\n";

    let hunks = merge_hunks(ancestor, ours, theirs);
    for h in &hunks {
        assert!(!matches!(h, Hunk::Conflict { .. }), "unexpected conflict");
    }
    let merged = round_trip(&hunks);
    assert!(contains(&merged, b"ours-only\n"));
}

#[test]
fn multiple_independent_conflicts_each_verbatim() {
    let ancestor = b"shared\nchange1\nshared2\nchange2\nshared3\n";
    let ours = b"shared\nours-c1\nshared2\nours-c2\nshared3\n";
    let theirs = b"shared\ntheirs-c1\nshared2\ntheirs-c2\nshared3\n";

    let hunks = merge_hunks(ancestor, ours, theirs);
    let conflicts: Vec<_> = hunks
        .iter()
        .filter(|h| matches!(h, Hunk::Conflict { .. }))
        .collect();
    assert!(conflicts.len() >= 2, "expected >=2 conflict hunks");
    for h in conflicts {
        let Hunk::Conflict {
            ours: c_ours,
            theirs: c_theirs,
        } = h
        else {
            unreachable!("filtered above")
        };
        assert!(contains(ours, c_ours));
        assert!(contains(theirs, c_theirs));
    }
}

#[test]
fn empty_inputs_yield_one_hunk_no_panic() {
    let hunks = merge_hunks(b"", b"", b"");
    assert!(!hunks.is_empty());
}

#[test]
fn identical_inputs_are_clean() {
    let content = b"line1\nline2\nline3\n";
    let hunks = merge_hunks(content, content, content);
    for h in &hunks {
        assert!(!matches!(h, Hunk::Conflict { .. }));
    }
}

// Both sides change different, non-adjacent regions of a CRLF/BOM file
// with no trailing newline: mixed-clean-merge fast path must preserve
// all three byte-faithfulness properties at once.
#[test]
fn mixed_clean_merge_preserves_crlf_bom_no_trailing_newline() {
    const BOM: &[u8] = b"\xef\xbb\xbf";
    let ancestor = [
        BOM,
        b"header\r\nancestor-line2\r\nsep\r\nancestor-line4" as &[u8],
    ]
    .concat();
    let ours = [
        BOM,
        b"header\r\nours-line2\r\nsep\r\nancestor-line4" as &[u8],
    ]
    .concat();
    let theirs = [
        BOM,
        b"header\r\nancestor-line2\r\nsep\r\ntheirs-line4" as &[u8],
    ]
    .concat();

    let hunks = merge_hunks(&ancestor, &ours, &theirs);
    assert_eq!(hunks.len(), 1);
    assert!(matches!(hunks[0], Hunk::Clean(_)));

    let merged = round_trip(&hunks);
    assert!(merged.starts_with(BOM), "BOM stripped");
    assert!(contains(&merged, b"\r\n"), "CRLF normalized away");
    assert_ne!(merged.last(), Some(&b'\n'), "trailing newline added");
    assert!(contains(&merged, b"ours-line2"));
    assert!(contains(&merged, b"theirs-line4"));
}

#[test]
fn parse_diff3_no_conflict_markers() {
    let output = b"line1\nline2\n";
    let parsed = parse_diff3(output, 7);
    assert_eq!(parsed.conflicts.len(), 0);
    assert_eq!(parsed.trailing_clean, output);
}

#[test]
fn clean_text_brackets_a_conflict_on_both_sides() {
    let ancestor = b"before\nshared\nafter\n";
    let ours = b"before\nours-change\nafter\n";
    let theirs = b"before\ntheirs-change\nafter\n";

    let hunks = merge_hunks(ancestor, ours, theirs);

    assert_eq!(
        hunks,
        vec![
            Hunk::Clean(b"before\n".to_vec()),
            Hunk::Conflict {
                ours: b"ours-change\n".to_vec(),
                theirs: b"theirs-change\n".to_vec(),
            },
            Hunk::Clean(b"after\n".to_vec()),
        ]
    );
}

#[test]
fn parse_diff3_conflict_sections() {
    let output =
        b"<<<<<<< ours\nours-line\n||||||| ancestor\nanc-line\n=======\ntheirs-line\n>>>>>>> theirs\n";
    let parsed = parse_diff3(output, 7);
    assert_eq!(parsed.conflicts.len(), 1);
    let (clean_before, block) = &parsed.conflicts[0];
    assert_eq!(block.ours, b"ours-line\n");
    assert_eq!(block.ancestor, b"anc-line\n");
    assert_eq!(block.theirs, b"theirs-line\n");
    assert!(clean_before.is_empty());
    assert!(parsed.trailing_clean.is_empty());
}

// A hand-built diff3 payload whose first conflict section names text
// that appears nowhere in `ours` (an anchor failure unrelated to the
// trailing-newline case), followed by a second conflict block that
// anchors normally. The widened fallback must swallow only the first
// block plus the clean text up to where the second block re-anchors —
// not the whole file — leaving the second conflict and the trailing
// clean region exactly as localized as they would be without any
// failure at all.
#[test]
fn unanchorable_section_widens_only_its_own_run() {
    let ours = b"AAAA\nBBBB\nshared\nCCCC\nDDDD\n";
    let theirs = b"AAAA\nXXXX\nshared\nYYYY\nDDDD\n";
    let diff3_output: &[u8] = b"AAAA\n\
<<<<<<< ours\nZZZZ\n||||||| ancestor\nanc1\n=======\nXXXX\n>>>>>>> theirs\n\
shared\n\
<<<<<<< ours\nCCCC\n||||||| ancestor\nanc2\n=======\nYYYY\n>>>>>>> theirs\n\
DDDD\n";

    let hunks = parse_hunks(ours, theirs, diff3_output, 7);

    assert_eq!(
        hunks,
        vec![
            Hunk::Conflict {
                ours: b"AAAA\nBBBB\nshared\n".to_vec(),
                theirs: b"AAAA\nXXXX\nshared\n".to_vec(),
            },
            Hunk::Conflict {
                ours: b"CCCC\n".to_vec(),
                theirs: b"YYYY\n".to_vec(),
            },
            Hunk::Clean(b"DDDD\n".to_vec()),
        ]
    );
}

#[test]
fn unanchorable_conflict_with_no_resync_runs_to_end_of_input() {
    let ancestor: &[u8] = b"eta\r\nBBBB\nCCCC\nzeta";
    let ours: &[u8] = b"AAAA\r\nBBBB\nCCCC\nAAAA";
    let theirs: &[u8] = b"eta\r\nBBBB\nCCCC\nXXXX";

    let hunks = merge_hunks(ancestor, ours, theirs);

    assert_eq!(
        hunks,
        vec![Hunk::Conflict {
            ours: ours.to_vec(),
            theirs: theirs.to_vec(),
        }]
    );
}
