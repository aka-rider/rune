//! Port of `golang/pkg/merge/hunks.go`: diffy identifies conflict
//! boundaries, then every hunk's bytes are re-anchored verbatim into the
//! original `ours`/`theirs` inputs. diffy's own serialized output is used
//! only to find where the boundaries are — never as buffer content.

use diffy::MergeOptions;

/// A classified region of a 3-way merge, in document order. Concatenating
/// `Clean` bytes and `Conflict::ours` bytes for each hunk reconstructs the
/// buffer content the merge should present verbatim.
///
/// Byte-faithfulness (§1.4.5): `Clean` bytes come verbatim from whichever
/// input contributed them; `Conflict` bytes come verbatim from the
/// respective `ours`/`theirs` inputs. Any region that cannot be re-anchored
/// this way degrades to a single whole-file `Conflict` rather than trusting
/// diffy's reserialized bytes (R5 fallback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hunk {
    /// Resolved bytes for this region — verbatim from ours or theirs.
    Clean(Vec<u8>),
    /// A region where both sides made different changes.
    Conflict {
        /// Verbatim ours bytes (kept in the buffer until resolved).
        ours: Vec<u8>,
        /// Verbatim theirs bytes (the `[T]` alternative).
        theirs: Vec<u8>,
    },
}

/// Performs a 3-way merge and returns the result as classified hunks.
///
/// Infallible and never empty: diffy's conflict case (`Err`) is not an
/// error, it is the conflict-marker payload used to locate boundaries; any
/// anchoring failure degrades to one whole-file [`Hunk::Conflict`] rather
/// than losing data.
pub fn merge_hunks(ancestor: &[u8], ours: &[u8], theirs: &[u8]) -> Vec<Hunk> {
    match MergeOptions::new().merge_bytes(ancestor, ours, theirs) {
        Ok(merged) => vec![classify_clean(ours, theirs, &merged)],
        Err(conflict_bytes) => parse_hunks(ours, theirs, &conflict_bytes),
    }
}

/// Classifies a conflict-free merge result (port of the Go reference's clean fast path).
/// The merged bytes are trusted only to pick which verbatim input to
/// return; diffy does not renormalize non-conflicting content.
fn classify_clean(ours: &[u8], theirs: &[u8], merged: &[u8]) -> Hunk {
    if merged == ours || ours == theirs {
        Hunk::Clean(ours.to_vec())
    } else if merged == theirs {
        Hunk::Clean(theirs.to_vec())
    } else {
        Hunk::Clean(merged.to_vec())
    }
}

/// The three sections of one diff3 conflict block, as raw line bytes.
#[derive(Default)]
struct Diff3Block {
    ours: Vec<u8>,
    ancestor: Vec<u8>,
    theirs: Vec<u8>,
}

enum Section {
    Ours,
    Ancestor,
    Theirs,
}

/// Returns the first line (including its line ending) and the remainder.
/// Empty input yields an empty line and an empty remainder.
fn next_line(b: &[u8]) -> (&[u8], &[u8]) {
    if b.is_empty() {
        return (b, b);
    }
    match b.iter().position(|&c| c == b'\n') {
        Some(i) => b.split_at(i + 1),
        None => (b, &[]),
    }
}

fn trim_end_crlf(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r\n")
        .or_else(|| line.strip_suffix(b"\n"))
        .unwrap_or(line)
}

fn is_ours_marker(line: &[u8]) -> bool {
    line.starts_with(b"<<<<<<<")
}

fn is_ancestor_marker(line: &[u8]) -> bool {
    line.starts_with(b"|||||||")
}

fn is_sep_marker(line: &[u8]) -> bool {
    trim_end_crlf(line) == b"======="
}

fn is_theirs_marker(line: &[u8]) -> bool {
    line.starts_with(b">>>>>>>")
}

/// Splits diffy's diff3 output into alternating clean segments and conflict
/// blocks. `cleans[i]` precedes `conflicts[i]`; `cleans.len() ==
/// conflicts.len() + 1` (port of the Go reference's `parseDiff3`).
fn parse_diff3(output: &[u8]) -> (Vec<Vec<u8>>, Vec<Diff3Block>) {
    let mut cleans = Vec::new();
    let mut conflicts = Vec::new();
    let mut current_clean = Vec::new();
    let mut remaining = output;

    while !remaining.is_empty() {
        let (line, rest) = next_line(remaining);
        remaining = rest;

        if !is_ours_marker(line) {
            current_clean.extend_from_slice(line);
            continue;
        }

        cleans.push(std::mem::take(&mut current_clean));

        let mut block = Diff3Block::default();
        let mut section = Section::Ours;
        while !remaining.is_empty() {
            let (cline, crest) = next_line(remaining);
            remaining = crest;
            if is_ancestor_marker(cline) {
                section = Section::Ancestor;
                continue;
            }
            if is_sep_marker(cline) {
                section = Section::Theirs;
                continue;
            }
            if is_theirs_marker(cline) {
                break;
            }
            match section {
                Section::Ours => block.ours.extend_from_slice(cline),
                Section::Ancestor => block.ancestor.extend_from_slice(cline),
                Section::Theirs => block.theirs.extend_from_slice(cline),
            }
        }
        conflicts.push(block);
    }

    cleans.push(current_clean);
    (cleans, conflicts)
}

/// A conflict block's sections re-anchored as byte ranges in the original
/// `ours`/`theirs` inputs.
struct Anchor {
    ours: (usize, usize),
    theirs: (usize, usize),
}

/// Finds `needle` verbatim in `haystack`, returning its byte offset.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Anchors one conflict section into `input` starting no earlier than
/// `search_from`, advancing the cursor so later blocks search forward only
/// (matches diff3's document-ordered, non-overlapping sections).
fn anchor_section(input: &[u8], search_from: usize, section: &[u8]) -> Option<(usize, usize)> {
    if section.is_empty() {
        return Some((search_from, search_from));
    }
    let haystack = input.get(search_from..)?;
    let idx = find_subslice(haystack, section)?;
    let start = search_from + idx;
    Some((start, start + section.len()))
}

/// Maps diff3 output boundaries back to verbatim ours/theirs bytes,
/// returning the classified hunk sequence (port of the Go
/// reference's `parseHunks`). Only called when diffy reports at least one
/// conflict; an empty `conflicts` list here is treated the same as a failed
/// anchor — degrade to one whole-file conflict rather than assume clean.
fn parse_hunks(ours: &[u8], theirs: &[u8], diff3_output: &[u8]) -> Vec<Hunk> {
    let (cleans, conflicts) = parse_diff3(diff3_output);

    let mut anchors = Vec::with_capacity(conflicts.len());
    let mut ours_search = 0usize;
    let mut theirs_search = 0usize;
    let mut valid = !conflicts.is_empty();

    for block in &conflicts {
        let Some(ours_range) = anchor_section(ours, ours_search, &block.ours) else {
            valid = false;
            break;
        };
        let Some(theirs_range) = anchor_section(theirs, theirs_search, &block.theirs) else {
            valid = false;
            break;
        };
        ours_search = ours_range.1;
        theirs_search = theirs_range.1;
        anchors.push(Anchor {
            ours: ours_range,
            theirs: theirs_range,
        });
    }

    if !valid {
        return vec![Hunk::Conflict {
            ours: ours.to_vec(),
            theirs: theirs.to_vec(),
        }];
    }

    let mut hunks = Vec::new();
    let mut ours_pos = 0usize;
    let mut theirs_pos = 0usize;

    for (i, clean) in cleans.iter().enumerate() {
        let (ours_clean_end, theirs_clean_end) = match anchors.get(i) {
            Some(a) => (a.ours.0, a.theirs.0),
            None => (ours.len(), theirs.len()),
        };

        let ours_clean = ours.get(ours_pos..ours_clean_end).unwrap_or(&[]);
        let theirs_clean = theirs.get(theirs_pos..theirs_clean_end).unwrap_or(&[]);

        if !ours_clean.is_empty() || !theirs_clean.is_empty() {
            hunks.push(classify_clean_region(ours_clean, theirs_clean, clean));
        }

        ours_pos = ours_clean_end;
        theirs_pos = theirs_clean_end;

        if let Some(a) = anchors.get(i) {
            hunks.push(Hunk::Conflict {
                ours: ours.get(a.ours.0..a.ours.1).unwrap_or(&[]).to_vec(),
                theirs: theirs.get(a.theirs.0..a.theirs.1).unwrap_or(&[]).to_vec(),
            });
            ours_pos = a.ours.1;
            theirs_pos = a.theirs.1;
        }
    }

    if hunks.is_empty() {
        hunks.push(Hunk::Clean(ours.to_vec()));
    }
    hunks
}

/// Classifies one clean region between conflicts (port of the Go reference's clean-region classification).
fn classify_clean_region(ours_clean: &[u8], theirs_clean: &[u8], merged_clean: &[u8]) -> Hunk {
    if ours_clean == theirs_clean {
        Hunk::Clean(ours_clean.to_vec())
    } else if theirs_clean == merged_clean {
        Hunk::Clean(theirs_clean.to_vec())
    } else if ours_clean == merged_clean {
        Hunk::Clean(ours_clean.to_vec())
    } else {
        Hunk::Conflict {
            ours: ours_clean.to_vec(),
            theirs: theirs_clean.to_vec(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
mod tests {
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
        let (cleans, conflicts) = parse_diff3(output);
        assert_eq!(conflicts.len(), 0);
        assert_eq!(cleans.len(), 1);
        assert_eq!(cleans[0], output);
    }

    #[test]
    fn parse_diff3_cleans_len_is_conflicts_plus_one() {
        let output = b"before\n<<<<<<< ours\nours-line\n||||||| ancestor\nanc-line\n=======\ntheirs-line\n>>>>>>> theirs\nafter\n";
        let (cleans, conflicts) = parse_diff3(output);
        assert_eq!(cleans.len(), conflicts.len() + 1);
    }

    #[test]
    fn parse_diff3_conflict_sections() {
        let output =
            b"<<<<<<< ours\nours-line\n||||||| ancestor\nanc-line\n=======\ntheirs-line\n>>>>>>> theirs\n";
        let (cleans, conflicts) = parse_diff3(output);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].ours, b"ours-line\n");
        assert_eq!(conflicts[0].ancestor, b"anc-line\n");
        assert_eq!(conflicts[0].theirs, b"theirs-line\n");
        assert!(cleans[0].is_empty());
        assert!(cleans[1].is_empty());
    }
}
