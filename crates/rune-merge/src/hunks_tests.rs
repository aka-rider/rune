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
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
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

// Under the old rendered-text anchoring this input defeated re-anchoring
// and collapsed to one whole-file conflict; position accounting localizes
// it: ours' clean first-line change auto-resolves, and only the genuinely
// double-edited last line conflicts.
#[test]
fn repeated_content_localizes_instead_of_collapsing_to_one_conflict() {
    let ancestor: &[u8] = b"eta\r\nBBBB\nCCCC\nzeta";
    let ours: &[u8] = b"AAAA\r\nBBBB\nCCCC\nAAAA";
    let theirs: &[u8] = b"eta\r\nBBBB\nCCCC\nXXXX";

    let hunks = merge_hunks(ancestor, ours, theirs);

    assert_eq!(
        hunks,
        vec![
            Hunk::Clean(b"AAAA\r\nBBBB\nCCCC\n".to_vec()),
            Hunk::Conflict {
                ours: b"AAAA".to_vec(),
                theirs: b"XXXX".to_vec(),
            },
        ]
    );
}

#[test]
fn crlf_file_with_disjoint_edits_merges_clean_with_exact_bytes() {
    let ancestor = b"alpha\r\nbeta\r\ngamma\r\ndelta\r\n";
    let ours = b"alpha\r\nours-beta\r\ngamma\r\ndelta\r\n";
    let theirs = b"alpha\r\nbeta\r\ngamma\r\ntheirs-delta\r\n";

    let hunks = merge_hunks(ancestor, ours, theirs);

    assert_eq!(
        hunks,
        vec![Hunk::Clean(
            b"alpha\r\nours-beta\r\ngamma\r\ntheirs-delta\r\n".to_vec()
        )]
    );
}

#[test]
fn ours_deletion_of_a_region_theirs_edited_conflicts_instead_of_vanishing() {
    let ancestor = b"keep\nold-a\nold-b\n";
    let ours = b"keep\n";
    let theirs = b"keep\nold-a\nnew-b\n";

    let hunks = merge_hunks(ancestor, ours, theirs);

    assert_eq!(
        hunks,
        vec![
            Hunk::Clean(b"keep\n".to_vec()),
            Hunk::Conflict {
                ours: b"".to_vec(),
                theirs: b"old-a\nnew-b\n".to_vec(),
            },
        ]
    );
}

#[test]
fn both_sides_insert_differently_at_the_same_point_conflicts() {
    let ancestor = b"top\nbottom\n";
    let ours = b"top\nours-mid\nbottom\n";
    let theirs = b"top\ntheirs-mid\nbottom\n";

    let hunks = merge_hunks(ancestor, ours, theirs);

    assert_eq!(
        hunks,
        vec![
            Hunk::Clean(b"top\n".to_vec()),
            Hunk::Conflict {
                ours: b"ours-mid\n".to_vec(),
                theirs: b"theirs-mid\n".to_vec(),
            },
            Hunk::Clean(b"bottom\n".to_vec()),
        ]
    );
}

#[test]
fn agreed_deletion_disappears_cleanly() {
    let ancestor = b"keep\ndrop\nkeep2\n";
    let ours = b"keep\nkeep2\n";
    let theirs = b"keep\nkeep2\n";

    let hunks = merge_hunks(ancestor, ours, theirs);

    assert_eq!(hunks, vec![Hunk::Clean(b"keep\nkeep2\n".to_vec())]);
}
