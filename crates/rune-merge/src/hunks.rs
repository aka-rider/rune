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
    let marker_len = conflict_marker_length(ancestor, ours, theirs);
    let mut opts = MergeOptions::new();
    opts.set_conflict_marker_length(marker_len);
    match opts.merge_bytes(ancestor, ours, theirs) {
        Ok(merged) => vec![classify_clean(ours, theirs, &merged)],
        Err(conflict_bytes) => parse_hunks(ours, theirs, &conflict_bytes, marker_len),
    }
}

const DEFAULT_CONFLICT_MARKER_LENGTH: usize = 7;

/// A conflict marker line collides with a document line that happens to
/// start with a run of the same repeated character diffy uses for markers
/// (`<`, `|`, `=`, `>`). Widening diffy's marker length past the longest
/// such run in any of the three inputs makes every marker diffy emits
/// longer than anything the document itself could produce, so line-prefix
/// matching against the rendered diff3 output can no longer confuse the
/// two.
fn conflict_marker_length(ancestor: &[u8], ours: &[u8], theirs: &[u8]) -> usize {
    let longest = [ancestor, ours, theirs]
        .into_iter()
        .map(longest_marker_like_run)
        .max()
        .unwrap_or(0);
    (longest + 1).max(DEFAULT_CONFLICT_MARKER_LENGTH)
}

fn longest_marker_like_run(bytes: &[u8]) -> usize {
    bytes
        .split(|&b| b == b'\n')
        .map(leading_repeated_run_len)
        .max()
        .unwrap_or(0)
}

fn leading_repeated_run_len(line: &[u8]) -> usize {
    match line.first() {
        Some(&marker) if matches!(marker, b'<' | b'|' | b'=' | b'>') => {
            line.iter().take_while(|&&b| b == marker).count()
        }
        _ => 0,
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

fn starts_with_repeated(line: &[u8], marker: u8, marker_len: usize) -> bool {
    line.get(..marker_len)
        .is_some_and(|run| run.iter().all(|&b| b == marker))
}

fn is_ours_marker(line: &[u8], marker_len: usize) -> bool {
    starts_with_repeated(line, b'<', marker_len)
}

fn is_ancestor_marker(line: &[u8], marker_len: usize) -> bool {
    starts_with_repeated(line, b'|', marker_len)
}

fn is_sep_marker(line: &[u8], marker_len: usize) -> bool {
    let trimmed = trim_end_crlf(line);
    trimmed.len() == marker_len && trimmed.iter().all(|&b| b == b'=')
}

fn is_theirs_marker(line: &[u8], marker_len: usize) -> bool {
    starts_with_repeated(line, b'>', marker_len)
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

fn parse_diff3(output: &[u8], marker_len: usize) -> Diff3Parse {
    let mut conflicts = Vec::new();
    let mut current_clean = Vec::new();
    let mut remaining = output;

    while !remaining.is_empty() {
        let (line, rest) = next_line(remaining);
        remaining = rest;

        if !is_ours_marker(line, marker_len) {
            current_clean.extend_from_slice(line);
            continue;
        }

        let clean_before = std::mem::take(&mut current_clean);

        let mut block = Diff3Block::default();
        let mut section = Section::Ours;
        while !remaining.is_empty() {
            let (cline, crest) = next_line(remaining);
            remaining = crest;
            if is_ancestor_marker(cline, marker_len) {
                section = Section::Ancestor;
                continue;
            }
            if is_sep_marker(cline, marker_len) {
                section = Section::Theirs;
                continue;
            }
            if is_theirs_marker(cline, marker_len) {
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
fn parse_hunks(ours: &[u8], theirs: &[u8], diff3_output: &[u8], marker_len: usize) -> Vec<Hunk> {
    let parsed = parse_diff3(diff3_output, marker_len);

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

fn empty_side_would_discard_nonempty(candidate: &[u8], other: &[u8]) -> bool {
    candidate.is_empty() && !other.is_empty()
}

fn classify_clean_region(ours_clean: &[u8], theirs_clean: &[u8], merged_clean: &[u8]) -> Hunk {
    if ours_clean == theirs_clean {
        return Hunk::Clean(ours_clean.to_vec());
    }
    if theirs_clean == merged_clean && !empty_side_would_discard_nonempty(theirs_clean, ours_clean)
    {
        return Hunk::Clean(theirs_clean.to_vec());
    }
    if ours_clean == merged_clean && !empty_side_would_discard_nonempty(ours_clean, theirs_clean) {
        return Hunk::Clean(ours_clean.to_vec());
    }
    Hunk::Conflict {
        ours: ours_clean.to_vec(),
        theirs: theirs_clean.to_vec(),
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
#[path = "hunks_tests.rs"]
mod tests;
