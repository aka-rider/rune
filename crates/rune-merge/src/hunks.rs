//! diffy identifies conflict boundaries, then every hunk's bytes are
//! re-anchored verbatim into the original `ours`/`theirs` inputs. diffy's
//! own serialized output is used only to find where the boundaries are —
//! never as buffer content.

use std::ops::Range;

use diffy::{DiffOptions, MergeOptions};

/// A classified region of a 3-way merge, in document order. Concatenating
/// `Clean` bytes and `Conflict::ours` bytes for each hunk reconstructs the
/// buffer content the merge should present verbatim.
///
/// Byte-faithfulness: `Clean` bytes come verbatim from whichever
/// input contributed them; `Conflict` bytes come verbatim from the
/// respective `ours`/`theirs` inputs. A section that cannot be re-anchored
/// widens its own conflict to swallow whatever lies between it and the next
/// point where anchoring resumes, rather than trusting diffy's reserialized
/// bytes there; only when nothing downstream anchors either does the whole
/// remaining file collapse into one `Conflict`.
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
/// error, it is the conflict-marker payload used to locate boundaries; an
/// anchoring failure widens only the affected conflict rather than losing
/// data or discarding localization elsewhere in the file.
pub fn merge_hunks(ancestor: &[u8], ours: &[u8], theirs: &[u8]) -> Vec<Hunk> {
    match MergeOptions::new().merge_bytes(ancestor, ours, theirs) {
        Ok(merged) => vec![classify_clean(ours, theirs, &merged)],
        Err(conflict_bytes) => parse_hunks(ours, theirs, &conflict_bytes),
    }
}

/// Classifies a conflict-free merge result. The merged bytes are trusted
/// only to pick which verbatim input to return; diffy does not
/// renormalize non-conflicting content.
fn classify_clean(ours: &[u8], theirs: &[u8], merged: &[u8]) -> Hunk {
    if ours == theirs {
        return Hunk::Clean(ours.to_vec());
    }
    if merged == ours {
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
    b.iter()
        .position(|&c| c == b'\n')
        .map_or((b, &[] as &[u8]), |i| b.split_at(i + 1))
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

/// The result of splitting diffy's diff3 output into alternating clean
/// segments and conflict blocks: each conflict is paired directly with the
/// clean segment immediately before it, so the two can never drift out of
/// step the way two index-aligned `Vec`s could.
#[derive(Default)]
struct Diff3Parse {
    conflicts: Vec<(Vec<u8>, Diff3Block)>,
    trailing_clean: Vec<u8>,
}

fn parse_diff3(output: &[u8]) -> Diff3Parse {
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

        let clean_before = std::mem::take(&mut current_clean);

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
        conflicts.push((clean_before, block));
    }

    Diff3Parse {
        conflicts,
        trailing_clean: current_clean,
    }
}

/// Finds `needle` verbatim in `haystack`, returning its byte offset.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn slice_or_empty(bytes: &[u8], range: Range<usize>) -> Vec<u8> {
    bytes.get(range).unwrap_or(&[]).to_vec()
}

/// Anchors one conflict section into `input` starting no earlier than
/// `search_from`, advancing the cursor so later blocks search forward only
/// (matches diff3's document-ordered, non-overlapping sections).
///
/// diffy's diff3 marker text is line-oriented and newline-terminates every
/// line it writes, including a section's final line when that line is also
/// the input's last line and the input itself has no trailing newline. A
/// verbatim search for the section as diffy wrote it then falls one byte
/// short at end-of-input. When the plain search fails, retry once with that
/// synthesized trailing newline stripped, accepting the match only when it
/// lands exactly at the end of `input` — the one place a missing trailing
/// newline can produce this mismatch.
fn anchor_section(input: &[u8], search_from: usize, section: &[u8]) -> Option<Range<usize>> {
    if section.is_empty() {
        return Some(search_from..search_from);
    }
    let haystack = input.get(search_from..)?;
    if let Some(idx) = find_subslice(haystack, section) {
        let start = search_from + idx;
        return Some(start..start + section.len());
    }
    let trimmed = section.strip_suffix(b"\n")?;
    let idx = find_subslice(haystack, trimmed)?;
    let start = search_from + idx;
    let end = start + trimmed.len();
    (end == input.len()).then_some(start..end)
}

struct OursSpan<'a> {
    bytes: &'a [u8],
    range: Range<usize>,
}

struct TheirsSpan<'a> {
    bytes: &'a [u8],
    range: Range<usize>,
}

/// Pushes the clean region between the last resolved position and the
/// upcoming conflict, unless both sides are empty there.
fn push_clean_region(
    hunks: &mut Vec<Hunk>,
    ours: OursSpan,
    theirs: TheirsSpan,
    merged_clean: &[u8],
) {
    let ours_clean = slice_or_empty(ours.bytes, ours.range);
    let theirs_clean = slice_or_empty(theirs.bytes, theirs.range);
    if !ours_clean.is_empty() || !theirs_clean.is_empty() {
        hunks.push(classify_clean_region(
            &ours_clean,
            &theirs_clean,
            merged_clean,
        ));
    }
}

fn find_resync(
    ours: &[u8],
    theirs: &[u8],
    ours_pos: usize,
    theirs_pos: usize,
    conflicts: &[(Vec<u8>, Diff3Block)],
    from: usize,
) -> Option<(usize, Range<usize>, Range<usize>)> {
    (from..conflicts.len()).find_map(|j| {
        let (_, next) = conflicts.get(j)?;
        let o = anchor_section(ours, ours_pos, &next.ours)?;
        let t = anchor_section(theirs, theirs_pos, &next.theirs)?;
        Some((j, o, t))
    })
}

/// Maps diff3 output boundaries back to verbatim ours/theirs bytes,
/// returning the classified hunk sequence. Only called when diffy reports
/// at least one conflict; an empty `conflicts` list here has no boundary to
/// re-anchor at all, so it degrades to one whole-file conflict rather than
/// assume clean.
///
/// Each conflict block is anchored independently, searching forward from
/// wherever the previous block left off. When a block's ours or theirs
/// section fails to anchor, the clean text and any further conflict blocks
/// between it and the next block that DOES anchor on both sides are folded
/// into that one widened conflict — the run of blocks that could not be
/// individually localized becomes one conflict spanning exactly that run,
/// leaving every other boundary in the file untouched. If nothing further
/// ever anchors, the widened conflict runs to the end of both inputs.
fn parse_hunks(ours: &[u8], theirs: &[u8], diff3_output: &[u8]) -> Vec<Hunk> {
    let parsed = parse_diff3(diff3_output);

    if parsed.conflicts.is_empty() {
        return vec![Hunk::Conflict {
            ours: ours.to_vec(),
            theirs: theirs.to_vec(),
        }];
    }

    let mut hunks = Vec::new();
    let mut ours_pos = 0usize;
    let mut theirs_pos = 0usize;
    let mut i = 0usize;

    while i < parsed.conflicts.len() {
        let Some((clean_before, block)) = parsed.conflicts.get(i) else {
            break;
        };
        let ours_range = anchor_section(ours, ours_pos, &block.ours);
        let theirs_range = anchor_section(theirs, theirs_pos, &block.theirs);

        if let (Some(o), Some(t)) = (ours_range, theirs_range) {
            push_clean_region(
                &mut hunks,
                OursSpan {
                    bytes: ours,
                    range: ours_pos..o.start,
                },
                TheirsSpan {
                    bytes: theirs,
                    range: theirs_pos..t.start,
                },
                clean_before,
            );
            let ours_end = o.end;
            let theirs_end = t.end;
            hunks.push(Hunk::Conflict {
                ours: slice_or_empty(ours, o),
                theirs: slice_or_empty(theirs, t),
            });
            ours_pos = ours_end;
            theirs_pos = theirs_end;
            i += 1;
            continue;
        }

        let resync = find_resync(ours, theirs, ours_pos, theirs_pos, &parsed.conflicts, i + 1);

        match resync {
            Some((j, o, t)) => {
                hunks.push(Hunk::Conflict {
                    ours: slice_or_empty(ours, ours_pos..o.start),
                    theirs: slice_or_empty(theirs, theirs_pos..t.start),
                });
                let ours_end = o.end;
                let theirs_end = t.end;
                hunks.push(Hunk::Conflict {
                    ours: slice_or_empty(ours, o),
                    theirs: slice_or_empty(theirs, t),
                });
                ours_pos = ours_end;
                theirs_pos = theirs_end;
                i = j + 1;
            }
            None => {
                hunks.push(Hunk::Conflict {
                    ours: slice_or_empty(ours, ours_pos..ours.len()),
                    theirs: slice_or_empty(theirs, theirs_pos..theirs.len()),
                });
                ours_pos = ours.len();
                theirs_pos = theirs.len();
                i = parsed.conflicts.len();
            }
        }
    }

    push_clean_region(
        &mut hunks,
        OursSpan {
            bytes: ours,
            range: ours_pos..ours.len(),
        },
        TheirsSpan {
            bytes: theirs,
            range: theirs_pos..theirs.len(),
        },
        &parsed.trailing_clean,
    );

    if hunks.is_empty() {
        hunks.push(Hunk::Clean(ours.to_vec()));
    }
    hunks
}

/// Classifies one clean region between conflicts.
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

/// Performs a 2-way merge for when there is no known common ancestor.
///
/// A 3-way diff3 with a synthesized empty ancestor cannot localize:
/// diffy's diff3 classifies a region as changed by comparing each side
/// against the ancestor, so an empty ancestor makes the entirety of both
/// `ours` and `theirs` count as changed, collapsing to one whole-file
/// conflict regardless of how much `ours` and `theirs` actually agree.
/// This instead runs a direct line diff between `ours` and `theirs`:
/// matching lines are `Clean`, and every differing run becomes a
/// `Conflict` — there is no ancestor to say which side is the "real"
/// change, so any disagreement is presented as one. diffy's line slices
/// are literal borrows of the input, so no re-anchoring is needed here.
pub fn merge_hunks_no_ancestor(ours: &[u8], theirs: &[u8]) -> Vec<Hunk> {
    let patch = DiffOptions::new()
        .set_context_len(usize::MAX)
        .create_patch_bytes(ours, theirs);

    let mut hunks = Vec::new();
    let mut clean_buf = Vec::new();
    let mut ours_buf = Vec::new();
    let mut theirs_buf = Vec::new();

    for line in patch.hunks().iter().flat_map(diffy::Hunk::lines) {
        match line {
            diffy::Line::Context(bytes) => {
                flush_no_ancestor_conflict(&mut hunks, &mut ours_buf, &mut theirs_buf);
                clean_buf.extend_from_slice(bytes);
            }
            diffy::Line::Delete(bytes) => {
                flush_no_ancestor_clean(&mut hunks, &mut clean_buf);
                ours_buf.extend_from_slice(bytes);
            }
            diffy::Line::Insert(bytes) => {
                flush_no_ancestor_clean(&mut hunks, &mut clean_buf);
                theirs_buf.extend_from_slice(bytes);
            }
        }
    }
    flush_no_ancestor_clean(&mut hunks, &mut clean_buf);
    flush_no_ancestor_conflict(&mut hunks, &mut ours_buf, &mut theirs_buf);

    if hunks.is_empty() {
        hunks.push(Hunk::Clean(ours.to_vec()));
    }
    hunks
}

fn flush_no_ancestor_clean(hunks: &mut Vec<Hunk>, clean_buf: &mut Vec<u8>) {
    if !clean_buf.is_empty() {
        hunks.push(Hunk::Clean(std::mem::take(clean_buf)));
    }
}

fn flush_no_ancestor_conflict(
    hunks: &mut Vec<Hunk>,
    ours_buf: &mut Vec<u8>,
    theirs_buf: &mut Vec<u8>,
) {
    if !ours_buf.is_empty() || !theirs_buf.is_empty() {
        hunks.push(Hunk::Conflict {
            ours: std::mem::take(ours_buf),
            theirs: std::mem::take(theirs_buf),
        });
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
        let parsed = parse_diff3(output);
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
        let parsed = parse_diff3(output);
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

        let hunks = parse_hunks(ours, theirs, diff3_output);

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
}
