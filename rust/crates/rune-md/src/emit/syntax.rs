//! `SyntaxSpan`/`SyntaxLine` (the Emitter's per-buffer-line output) and
//! `SyntaxSnapshot` (Buffer Space <-> Syntax Space coordinate conversion) —
//! port of `pkg/editor/display/syntax_snapshot.go:35-97`.

use std::ops::Range;

use crate::emit::style::StyleId;
use rune_core::coords::{BufferPoint, SyntaxPoint};

/// Per-visual-char buffer offset, `-1` for decorative/padding cells with no
/// buffer correspondence — port of `pkg/editor/display/cellmap.go`'s
/// `CellMapping`. Phase 1 never produces `-1` (no decorative padding in this
/// crate yet); the type still carries it so the proptest invariant
/// ("entries are -1 or valid char boundaries") is meaningful, and so a
/// future decorative producer doesn't need a type change.
pub type CellMap = Vec<i64>;

/// Port of `pkg/editor/display/syntax_map.go`'s per-run span, reshaped into
/// two variants (plan Context: "Span becomes an enum ... Makes 'identical
/// text carrying a cell map' unrepresentable") so a producer can no longer
/// pair a `cell_map` with text that's already a verbatim buffer slice, nor
/// omit one from text that isn't.
#[derive(Clone, Debug)]
pub enum SyntaxSpan {
    /// The visible text is a direct, verbatim slice of the buffer at
    /// `range` — no delimiter bytes were dropped or substituted. Its text
    /// is always recoverable as `&content[range]` (see [`SyntaxSpan::text`]);
    /// no `cell_map` is needed since buffer position and visible position
    /// coincide one-to-one.
    Identical { style: StyleId, range: Range<usize> },
    /// The visible `text` differs from the buffer at `range` (concealed
    /// marker/delimiter bytes were dropped from what's shown), so it carries
    /// its own text plus a `cell_map` mapping each visible char back to its
    /// buffer offset.
    Substituted {
        style: StyleId,
        text: String,
        range: Range<usize>,
        cell_map: CellMap,
    },
}

impl SyntaxSpan {
    pub fn style(&self) -> StyleId {
        match self {
            SyntaxSpan::Identical { style, .. } | SyntaxSpan::Substituted { style, .. } => *style,
        }
    }

    pub fn range(&self) -> Range<usize> {
        match self {
            SyntaxSpan::Identical { range, .. } | SyntaxSpan::Substituted { range, .. } => {
                range.clone()
            }
        }
    }

    pub fn is_rendered(&self) -> bool {
        matches!(self, SyntaxSpan::Substituted { .. })
    }

    /// The span's visible text. `Identical` recovers it verbatim from
    /// `content` at `range`; `Substituted` returns its own stored `text`
    /// (which is not, in general, `content[range]` — that range may still
    /// cover dropped delimiter bytes, module docs in `wrap.rs`).
    pub fn text<'a>(&'a self, content: &'a str) -> &'a str {
        match self {
            SyntaxSpan::Identical { range, .. } => content.get(range.clone()).unwrap_or(""),
            SyntaxSpan::Substituted { text, .. } => text.as_str(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SyntaxLine {
    pub spans: Vec<SyntaxSpan>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct OffsetDelta {
    pub(crate) buffer_offset: usize,
    pub(crate) delta: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HiddenRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) clamp_to: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LineConversion {
    pub(crate) deltas: Vec<OffsetDelta>,
    pub(crate) hidden: Vec<HiddenRange>,
}

/// Coordinate conversion between Buffer Space and Syntax Space — port of
/// `pkg/editor/display/syntax_snapshot.go:35-97`. Positions inside hidden
/// delimiters clamp to the nearest cursor-legal syntax position.
#[derive(Clone, Debug, Default)]
pub struct SyntaxSnapshot {
    pub(crate) line_convs: Vec<LineConversion>,
}

impl SyntaxSnapshot {
    /// Sum of hidden-range byte lengths recorded for `line`. Exposed for the
    /// per-line coverage invariant test (every byte is either a visible
    /// span or a hidden range — never silently dropped) and useful to any
    /// future caller that wants to know how many raw markup bytes a line is
    /// currently concealing.
    pub fn hidden_byte_count(&self, line: usize) -> usize {
        self.line_convs
            .get(line)
            .map(|lc| {
                lc.hidden
                    .iter()
                    .map(|h| h.end.saturating_sub(h.start))
                    .sum()
            })
            .unwrap_or(0)
    }

    pub fn buffer_to_syntax(&self, bp: BufferPoint) -> SyntaxPoint {
        let Some(lc) = self.line_convs.get(bp.line) else {
            return SyntaxPoint {
                line: bp.line,
                col: bp.col,
            };
        };
        if lc.deltas.is_empty() {
            return SyntaxPoint {
                line: bp.line,
                col: bp.col,
            };
        }
        let col = clamp_col(bp.col, &lc.hidden);
        let mut delta = 0usize;
        for d in &lc.deltas {
            if d.buffer_offset <= col {
                delta = d.delta;
            } else {
                break;
            }
        }
        SyntaxPoint {
            line: bp.line,
            col: col.saturating_sub(delta),
        }
    }

    pub fn syntax_to_buffer(&self, sp: SyntaxPoint) -> BufferPoint {
        let Some(lc) = self.line_convs.get(sp.line) else {
            return BufferPoint {
                line: sp.line,
                col: sp.col,
            };
        };
        if lc.deltas.is_empty() {
            return BufferPoint {
                line: sp.line,
                col: sp.col,
            };
        }
        let mut delta = 0usize;
        for d in &lc.deltas {
            let syntax_at_entry = d.buffer_offset.saturating_sub(d.delta);
            if syntax_at_entry <= sp.col {
                delta = d.delta;
            } else {
                break;
            }
        }
        BufferPoint {
            line: sp.line,
            col: sp.col + delta,
        }
    }
}

fn clamp_col(col: usize, hidden: &[HiddenRange]) -> usize {
    for h in hidden {
        if col >= h.start && col < h.end {
            return h.clamp_to;
        }
        if h.start > col {
            break;
        }
    }
    col
}

/// `true` iff `sorted` (already ordered by start) contains a genuine
/// overlap — two ranges sharing at least one byte. Adjacent-but-touching
/// ranges (`end == next.start`) are NOT an overlap. Used only by the
/// `STRICT_INVARIANTS`-gated assert in `build_line_conversions`: every
/// hidden-range producer in this crate is expected to already emit
/// disjoint ranges, so an overlap here means a producer bug (the exact
/// shape two separate findings on this branch turned out to be — a
/// fence's ranges colliding with its container's marker ranges) — this
/// makes it surface in tests instead of being silently absorbed by the
/// merge below.
fn has_overlap(sorted: &[(usize, usize)]) -> bool {
    sorted.windows(2).any(|w| match w {
        [(_, prev_end), (next_start, _)] => prev_end > next_start,
        _ => false,
    })
}

/// Merges overlapping or touching-adjacent intervals in `sorted` (already
/// ordered by start) into the minimal disjoint set covering the same
/// bytes. The chokepoint that makes "a byte is hidden at most once"
/// structural rather than every producer's responsibility: even if some
/// future producer reintroduces an overlapping-range bug (the class two
/// separate findings on this branch belonged to), the delta accumulation
/// below can no longer double-count it — merging happens UNCONDITIONALLY
/// in every build (§1.3 graceful degradation); the `STRICT_INVARIANTS`
/// assert above is what surfaces the producer bug, in tests only. Also
/// reused by `emit::unclaimed_subranges` for the visible-side counterpart
/// of this same collapse.
pub(crate) fn merge_overlapping(sorted: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(sorted.len());
    for (s, e) in sorted {
        if e <= s {
            continue;
        }
        if let Some(last) = merged.last_mut()
            && s <= last.1
        {
            last.1 = last.1.max(e);
            continue;
        }
        merged.push((s, e));
    }
    merged
}

/// Builds the per-line `LineConversion` table from each line's raw hidden
/// byte ranges (absolute buffer offsets) — the accumulating-delta model
/// `SyntaxSnapshot::buffer_to_syntax`/`syntax_to_buffer` read. Every byte is
/// hidden AT MOST ONCE by construction: overlapping/touching ranges are
/// merged before the deltas are summed (see `merge_overlapping`), so a
/// producer bug that hands back overlapping ranges degrades to a
/// (test-asserted, see `has_overlap` and `crate::emit::STRICT_INVARIANTS`)
/// coordinate inaccuracy rather than a doubly-counted delta corrupting
/// every position past it on the line.
pub(crate) fn build_line_conversions(
    starts: &[usize],
    hidden: &[Vec<(usize, usize)>],
) -> Vec<LineConversion> {
    let mut convs = Vec::with_capacity(hidden.len());
    for (line, ranges) in hidden.iter().enumerate() {
        let line_start = starts.get(line).copied().unwrap_or(0);
        let mut rel: Vec<(usize, usize)> = ranges
            .iter()
            .filter(|&&(s, e)| e > s)
            .map(|&(s, e)| (s.saturating_sub(line_start), e.saturating_sub(line_start)))
            .collect();
        rel.sort_by_key(|&(s, _)| s);

        // §1.3: never panics in an ordinary shipped build — only in tests
        // (or a build that opts in via the `strict-invariants` feature),
        // via the shared `assert_invariant` chokepoint. The merge two
        // lines below runs unconditionally regardless.
        crate::emit::assert_invariant(!has_overlap(&rel), || {
            format!("line {line}: overlapping hidden ranges from a producer bug: {rel:?}")
        });

        let merged = merge_overlapping(rel);

        let mut deltas = Vec::with_capacity(merged.len());
        let mut hidden_ranges = Vec::with_capacity(merged.len());
        let mut accum = 0usize;
        for (s, e) in merged {
            accum += e - s;
            hidden_ranges.push(HiddenRange {
                start: s,
                end: e,
                clamp_to: e,
            });
            deltas.push(OffsetDelta {
                buffer_offset: e,
                delta: accum,
            });
        }
        convs.push(LineConversion {
            deltas,
            hidden: hidden_ranges,
        });
    }
    convs
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// The structural-hardening chokepoint: overlapping input intervals
    /// merge into the minimal disjoint set covering the SAME bytes exactly
    /// once — an overlapping-range producer bug degrades to a coordinate
    /// inaccuracy (caught separately by the `STRICT_INVARIANTS`-gated
    /// assert below, in tests), never a doubly-counted delta.
    #[test]
    fn merge_overlapping_intervals_counts_each_byte_once() {
        // [0,5) and [3,8) share bytes [3,5) — merged into one [0,8): 8
        // bytes total, NOT 5+5=10 (which is what a naive unmerged sum
        // would produce, the exact "delta summed twice" shape reported for
        // a fence's ranges colliding with its container's marker ranges).
        let merged = merge_overlapping(vec![(0, 5), (3, 8), (10, 12)]);
        assert_eq!(merged, vec![(0, 8), (10, 12)]);
        let total_bytes: usize = merged.iter().map(|&(s, e)| e - s).sum();
        assert_eq!(total_bytes, 10); // 8 (merged) + 2, not 5+5+2=12

        // Touching-but-not-overlapping ranges also merge (no shared byte,
        // but no gap either): [0,4) and [4,9).
        let touching = merge_overlapping(vec![(0, 4), (4, 9)]);
        assert_eq!(touching, vec![(0, 9)]);
    }

    #[test]
    fn has_overlap_distinguishes_overlap_from_mere_adjacency() {
        assert!(has_overlap(&[(0, 5), (3, 8)])); // shares bytes [3,5)
        assert!(!has_overlap(&[(0, 4), (4, 9)])); // touches at 4, no shared byte
        assert!(!has_overlap(&[(0, 2), (5, 7)])); // disjoint with a gap
        assert!(!has_overlap(&[]));
    }

    /// Proves the `STRICT_INVARIANTS`-gated assert in
    /// `build_line_conversions` is actually wired to fire on overlapping
    /// input — the two prior findings on this branch (a fence's ranges
    /// colliding with its container's marker ranges) were both this exact
    /// shape, and would have tripped this assertion in tests immediately
    /// instead of silently corrupting coordinate conversion. Unlike the
    /// old `debug_assert!`-based version, `STRICT_INVARIANTS` is tied to
    /// `cfg(test)` (not `cfg(debug_assertions)`), so this fires in a
    /// `--release` test run too (§1.3: the assert is test-only, not
    /// profile-only — a `cargo test --release` run must still catch this).
    #[test]
    #[should_panic(expected = "overlapping hidden ranges")]
    fn build_line_conversions_debug_asserts_on_overlapping_input() {
        // Two overlapping ranges on line 0: [0,5) and [3,8).
        let starts = vec![0usize];
        let hidden = vec![vec![(0usize, 5usize), (3usize, 8usize)]];
        let _ = build_line_conversions(&starts, &hidden);
    }
}
