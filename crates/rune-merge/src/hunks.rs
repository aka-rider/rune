//! Three-way merge classification built from two structured line diffs
//! (ancestor→ours and ancestor→theirs), both keyed to ancestor line
//! positions, aligned by position accounting. Every byte returned in a
//! [`Hunk`] is a verbatim slice of `ours`, `theirs`, or a region all
//! sides agree on — no rendered merge text is ever parsed or trusted.

use std::ops::Range;

use diffy::DiffOptions;

/// A classified region of a 3-way merge, in document order. Concatenating
/// `Clean` bytes and `Conflict::ours` bytes for each hunk reconstructs the
/// buffer content the merge should present verbatim.
///
/// A `Clean` region carries whichever side's change the merge
/// auto-resolved there (or the shared bytes when nobody changed them); a
/// `Conflict` carries both sides' verbatim bytes for a region both sides
/// changed differently. A side is declared unchanged only by byte
/// equality with the ancestor over the exact region, so discarding one
/// side's non-empty edit as "clean" is unrepresentable.
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
/// Infallible and never empty.
pub fn merge_hunks(ancestor: &[u8], ours: &[u8], theirs: &[u8]) -> Vec<Hunk> {
    let anc_lines = split_lines(ancestor);
    let ours_edits = side_edits(ancestor, ours);
    let theirs_edits = side_edits(ancestor, theirs);
    let walked = chunk_walk(&anc_lines, &ours_edits, &theirs_edits);
    let mut hunks = coalesce_clean(walked);
    if hunks.is_empty() {
        hunks.push(Hunk::Clean(Vec::new()));
    }
    hunks
}

/// Splits after every `\n`, keeping the terminator; an unterminated tail
/// is its own line. This is the same rule diffy's line iterator uses, so
/// the edit positions reported by the patches index into these lines.
fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let end = rest
            .iter()
            .position(|&b| b == b'\n')
            .map_or(rest.len(), |i| i + 1);
        let (line, remaining) = rest.split_at(end);
        lines.push(line);
        rest = remaining;
    }
    lines
}

/// One side's deviation from the ancestor: the ancestor lines it removes
/// (possibly none, for a pure insertion) and the bytes it puts there.
struct SideEdit {
    anc: Range<usize>,
    replacement: Vec<u8>,
}

fn side_edits(ancestor: &[u8], side: &[u8]) -> Vec<SideEdit> {
    let patch = DiffOptions::new().create_patch_bytes(ancestor, side);
    let mut edits = Vec::new();
    for hunk in patch.hunks() {
        let old = hunk.old_range();
        let mut at = if old.is_empty() {
            old.start()
        } else {
            old.start().saturating_sub(1)
        };
        let mut run: Option<SideEdit> = None;
        for line in hunk.lines() {
            match line {
                diffy::Line::Context(_) => {
                    if let Some(edit) = run.take() {
                        edits.push(edit);
                    }
                    at += 1;
                }
                diffy::Line::Delete(_) => {
                    let edit = run.get_or_insert_with(|| SideEdit {
                        anc: at..at,
                        replacement: Vec::new(),
                    });
                    edit.anc.end = at + 1;
                    at += 1;
                }
                diffy::Line::Insert(bytes) => {
                    let edit = run.get_or_insert_with(|| SideEdit {
                        anc: at..at,
                        replacement: Vec::new(),
                    });
                    edit.replacement.extend_from_slice(bytes);
                }
            }
        }
        if let Some(edit) = run.take() {
            edits.push(edit);
        }
    }
    edits
}

/// Walks the ancestor once, grouping overlapping edits from the two sides
/// into chunks and classifying each chunk by byte equality.
///
/// Grouping rule: a chunk seeded by one edit absorbs any later edit whose
/// ancestor range shares a line with the chunk's span; a zero-width edit
/// (pure insertion at a line boundary) joins only a chunk whose span
/// strictly contains its insertion point, or another zero-width edit at
/// the same point. Insertions at a chunk boundary therefore stay their
/// own chunk, ordered before the lines they precede.
fn chunk_walk(
    anc_lines: &[&[u8]],
    ours_edits: &[SideEdit],
    theirs_edits: &[SideEdit],
) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut emitted = 0usize;
    let mut next_ours = 0usize;
    let mut next_theirs = 0usize;

    while next_ours < ours_edits.len() || next_theirs < theirs_edits.len() {
        let chunk_start_ours = next_ours;
        let chunk_start_theirs = next_theirs;
        let take_ours = match (ours_edits.get(next_ours), theirs_edits.get(next_theirs)) {
            (Some(o), Some(t)) => (o.anc.start, o.anc.end) <= (t.anc.start, t.anc.end),
            (Some(_), None) => true,
            _ => false,
        };
        let seed = if take_ours {
            let edit = ours_edits.get(next_ours);
            next_ours += 1;
            edit
        } else {
            let edit = theirs_edits.get(next_theirs);
            next_theirs += 1;
            edit
        };
        let Some(seed) = seed else {
            break;
        };
        let mut span = seed.anc.clone();

        loop {
            let ours_cand = ours_edits
                .get(next_ours)
                .filter(|e| overlaps(&span, &e.anc));
            let theirs_cand = theirs_edits
                .get(next_theirs)
                .filter(|e| overlaps(&span, &e.anc));
            match (ours_cand, theirs_cand) {
                (Some(o), Some(t)) => {
                    if (o.anc.start, o.anc.end) <= (t.anc.start, t.anc.end) {
                        span = union(&span, &o.anc);
                        next_ours += 1;
                    } else {
                        span = union(&span, &t.anc);
                        next_theirs += 1;
                    }
                }
                (Some(o), None) => {
                    span = union(&span, &o.anc);
                    next_ours += 1;
                }
                (None, Some(t)) => {
                    span = union(&span, &t.anc);
                    next_theirs += 1;
                }
                (None, None) => break,
            }
        }

        if span.start > emitted {
            hunks.push(Hunk::Clean(concat_lines(anc_lines, emitted..span.start)));
        }
        let ours_bytes = side_span_bytes(
            anc_lines,
            &span,
            ours_edits.get(chunk_start_ours..next_ours).unwrap_or(&[]),
        );
        let theirs_bytes = side_span_bytes(
            anc_lines,
            &span,
            theirs_edits
                .get(chunk_start_theirs..next_theirs)
                .unwrap_or(&[]),
        );
        let anc_bytes = concat_lines(anc_lines, span.clone());
        hunks.push(classify_chunk(ours_bytes, theirs_bytes, &anc_bytes));
        emitted = span.end.max(emitted);
    }

    if emitted < anc_lines.len() {
        hunks.push(Hunk::Clean(concat_lines(
            anc_lines,
            emitted..anc_lines.len(),
        )));
    }
    hunks
}

fn overlaps(span: &Range<usize>, edit: &Range<usize>) -> bool {
    match (span.is_empty(), edit.is_empty()) {
        (false, false) => span.start < edit.end && edit.start < span.end,
        (false, true) => span.start < edit.start && edit.start < span.end,
        (true, false) => edit.start < span.start && span.start < edit.end,
        (true, true) => span.start == edit.start,
    }
}

fn union(a: &Range<usize>, b: &Range<usize>) -> Range<usize> {
    a.start.min(b.start)..a.end.max(b.end)
}

fn concat_lines(anc_lines: &[&[u8]], range: Range<usize>) -> Vec<u8> {
    anc_lines
        .get(range)
        .unwrap_or(&[])
        .iter()
        .flat_map(|line| line.iter().copied())
        .collect()
}

/// Reconstructs one side's bytes over an ancestor span: kept ancestor
/// lines verbatim, with each of the side's edits substituted in place.
fn side_span_bytes(anc_lines: &[&[u8]], span: &Range<usize>, edits: &[SideEdit]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = span.start;
    for edit in edits {
        let kept = at..edit.anc.start.max(at);
        out.extend(concat_lines(anc_lines, kept));
        out.extend_from_slice(&edit.replacement);
        at = edit.anc.end.max(at);
    }
    out.extend(concat_lines(anc_lines, at..span.end.max(at)));
    out
}

/// A side counts as unchanged only when its bytes equal the ancestor's
/// over this exact span, so a non-empty edit can never be discarded by an
/// accidental equality between two unrelated empty strings.
fn classify_chunk(ours_bytes: Vec<u8>, theirs_bytes: Vec<u8>, anc_bytes: &[u8]) -> Hunk {
    if ours_bytes == theirs_bytes {
        Hunk::Clean(ours_bytes)
    } else if ours_bytes == anc_bytes {
        Hunk::Clean(theirs_bytes)
    } else if theirs_bytes == anc_bytes {
        Hunk::Clean(ours_bytes)
    } else {
        Hunk::Conflict {
            ours: ours_bytes,
            theirs: theirs_bytes,
        }
    }
}

fn coalesce_clean(walked: Vec<Hunk>) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    for hunk in walked {
        match hunk {
            Hunk::Clean(bytes) => {
                if bytes.is_empty() {
                    continue;
                }
                if let Some(Hunk::Clean(tail)) = hunks.last_mut() {
                    tail.extend_from_slice(&bytes);
                } else {
                    hunks.push(Hunk::Clean(bytes));
                }
            }
            conflict => hunks.push(conflict),
        }
    }
    hunks
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
