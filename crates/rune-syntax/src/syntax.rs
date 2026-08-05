//! `SyntaxSpan`/`SyntaxLine` (the Emitter's per-buffer-line output) and
//! `SyntaxSnapshot` (Buffer Space <-> Syntax Space coordinate conversion).
//! Producer-agnostic (WP3): `rune-md`'s emitter is the only producer today,
//! but nothing here depends on markdown.

use std::ops::Range;

use crate::scope::ScopeId;
use rune_core::coords::{BufferPoint, SyntaxPoint};

/// Mirrors `rune-md`'s own `STRICT_INVARIANTS`/`assert_invariant`
/// chokepoint: `true` only in test builds or when this
/// crate's own `strict-invariants` feature is explicitly enabled. Kept as a
/// local copy rather than a shared helper — each crate's gate governs only
/// its own producer-bug invariants, and `rune-syntax` must stand alone
/// without depending back on `rune-md`.
const STRICT_INVARIANTS: bool = cfg!(any(test, feature = "strict-invariants"));

pub(crate) fn assert_invariant(cond: bool, msg: impl FnOnce() -> String) {
    if STRICT_INVARIANTS {
        assert!(cond, "{}", msg());
    }
}

/// The nearest char boundary at or BEFORE `idx` — a safe, stable-Rust
/// equivalent of the nightly-only `str::floor_char_boundary`. Never panics:
/// `is_char_boundary` is a plain byte-position check, and a UTF-8 char is at
/// most 4 bytes, so the loop always terminates within a few iterations. A
/// private copy rather than a shared helper: this crate must stand alone
/// without depending back on `rune-md`, which needs the identical logic for
/// its own producer-side snapping.
fn floor_char_boundary(s: &str, idx: usize) -> usize {
    let mut idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// The nearest char boundary at or AFTER `idx` — the stable-Rust equivalent
/// of the nightly-only `str::ceil_char_boundary`.
fn ceil_char_boundary(s: &str, idx: usize) -> usize {
    let mut idx = idx.min(s.len());
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

/// Per-visual-char buffer offset, `-1` for decorative/padding cells with no
/// buffer correspondence. `rune-md`'s synthesized table-border rows already
/// produce all-`-1` maps (a border
/// row's text is decorative, with no buffer position to point back to);
/// this crate itself never constructs one, but must accept whatever a
/// producer hands it.
pub type CellMap = Vec<i64>;

/// A per-run syntax-map span, modeled as
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
    Identical { scope: ScopeId, range: Range<usize> },
    /// The visible `text` differs from the buffer at `range` (concealed
    /// marker/delimiter bytes were dropped from what's shown), so it carries
    /// its own text plus a `cell_map` mapping each visible char back to its
    /// buffer offset.
    Substituted {
        scope: ScopeId,
        text: String,
        range: Range<usize>,
        cell_map: CellMap,
    },
}

impl SyntaxSpan {
    /// Checked constructor for the `Identical` variant: clamps `range` to
    /// `content`'s length and to the nearest char boundaries at
    /// construction time, so `range` is always a valid slice of `content`
    /// — making it structurally impossible for `span_visible_len`
    /// (`range.end - range.start`) to disagree with `text()`
    /// (`content.get(range)`, whose `unwrap_or("")` fallback this
    /// guarantees never actually fires) the way an externally-guarded
    /// convention could still get wrong. A producer handing back an
    /// out-of-bounds or mid-codepoint range is a bug; this degrades it to a
    /// clamped (never panicking) range in every build, surfaced via
    /// `assert_invariant` in tests.
    pub fn identical(content: &str, scope: ScopeId, range: Range<usize>) -> SyntaxSpan {
        let len = content.len();
        let start = range.start.min(len);
        let end = range.end.min(len).max(start);
        let snapped_start = floor_char_boundary(content, start);
        let snapped_end = ceil_char_boundary(content, end).max(snapped_start);
        assert_invariant(
            snapped_start == range.start && snapped_end == range.end,
            || {
                format!(
                    "Identical span {:?} is out of bounds or not on a char boundary for a {len}-byte buffer — clamped to [{snapped_start},{snapped_end})",
                    range
                )
            },
        );
        SyntaxSpan::Identical {
            scope,
            range: snapped_start..snapped_end,
        }
    }

    /// The scope this span is tagged with (WP4: replaces `StyleId`) — a
    /// theme resolves it to a rendered `Style`; this crate never does.
    pub fn scope(&self) -> ScopeId {
        match self {
            SyntaxSpan::Identical { scope, .. } | SyntaxSpan::Substituted { scope, .. } => *scope,
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
    /// `content` at `range` — a range built through [`SyntaxSpan::identical`]
    /// is always a valid slice of `content` by construction, so the
    /// `unwrap_or("")` fallback below is defense-in-depth, never the live
    /// path; `Substituted` returns its own stored `text` (which is not, in
    /// general, `content[range]` — a wrap break can narrow that text
    /// without narrowing `range`, the wrap pass's own doing, not this
    /// module's).
    pub fn text<'a>(&'a self, content: &'a str) -> &'a str {
        match self {
            SyntaxSpan::Identical { range, .. } => content.get(range.clone()).unwrap_or(""),
            SyntaxSpan::Substituted { text, .. } => text.as_str(),
        }
    }
}

/// Which row of a rendered table's Grid a `SyntaxLine` carries — feeds
/// `markup.table.header` vs `markup.table.separator` vs body-role styling
/// (WP2.S7/S8's producer, not this step's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableRole {
    Header,
    Separator,
    Body,
}

/// Where a row sits among a table's rendered rows — the synthesised
/// top/bottom/inter-row border a `DisplaySnapshot` inserts around it (WP3)
/// depends on which edges are already the table's own start/end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowBoundary {
    Only,
    First,
    Middle,
    Last,
}

/// Table geometry a rendered row's source `SyntaxLine` carries directly
/// (architectural decision 7: "one explicit field removes the illegal
/// state instead of guarding it" — no consumer has to sniff every span's
/// scope to tell a table row from prose). Populated by `rune-md`'s table
/// producer for every Rendered table line (Grid/Wrapped/Pivoted layout);
/// `None` for a Revealed table line, which has raw markup, not rendered
/// geometry, to describe.
#[derive(Clone, Debug)]
pub struct TableRowInfo {
    pub col_widths: Vec<usize>,
    pub role: TableRole,
    pub boundary: RowBoundary,
    /// Visual rows 2..N of this source line (Wrapped/Pivoted only). Row 1
    /// is `SyntaxLine::spans`. These claim no bytes — they never enter
    /// `line.spans`, so neither the emitter's gap-filler nor its per-line
    /// span sort ever sees them, which is what keeps a table line's
    /// visible-plus-hidden byte accounting whole.
    pub extra_rows: Vec<Vec<SyntaxSpan>>,
    /// Whether this table draws a box around itself. Grid and Wrapped do;
    /// the Pivoted key-value layout deliberately has no box at all, and a
    /// consumer that synthesises border rows must not draw them around one.
    /// A bool rather than a layout enum: this is the only thing the display
    /// pass actually asks, and the layout kind itself lives in the producer.
    pub boxed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SyntaxLine {
    pub spans: Vec<SyntaxSpan>,
    /// `Some` only for a rendered table row's source line — `None` for
    /// every other line, including a Revealed table line (raw markdown,
    /// no rendered geometry to describe).
    pub table: Option<TableRowInfo>,
    /// Decorative glyphs (heading icon, list bullet/number, quote bar, hr
    /// rule) a Rendered line carries out-of-band from its spans — see
    /// `crate::decor::LineDecor`. `None` for a Revealed line (raw markup is
    /// already visible, nothing to decorate) and for a table line (table
    /// geometry is described by `table`, never by decor).
    pub decor: Option<crate::decor::LineDecor>,
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

/// Coordinate conversion between Buffer Space and Syntax Space. Positions
/// inside hidden delimiters clamp to the nearest cursor-legal syntax
/// position.
#[derive(Clone, Debug, Default)]
pub struct SyntaxSnapshot {
    pub(crate) line_convs: Vec<LineConversion>,
}

impl SyntaxSnapshot {
    /// Builds a `SyntaxSnapshot` from a producer's raw per-line hidden-range
    /// lists (see [`build_line_conversions`] for the accumulating-delta
    /// model). The one entry point a cross-crate producer (`rune-md`'s
    /// `emit()` today) uses instead of a `line_convs` struct literal — that
    /// field is `pub(crate)` to this crate on purpose, so every conversion
    /// table is built through the overlap-checked path below.
    pub fn build(starts: &[usize], hidden: &[Vec<(usize, usize)>]) -> SyntaxSnapshot {
        SyntaxSnapshot {
            line_convs: build_line_conversions(starts, hidden),
        }
    }

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

/// `true` iff `intervals` (any order) contains a genuine overlap — two
/// ranges sharing at least one byte. Adjacent-but-touching ranges
/// (`end == next.start`) are NOT an overlap. Sorts a local copy rather than
/// requiring the caller to hand back ordered input — this runs only behind
/// the `STRICT_INVARIANTS`-gated assert in `build_line_conversions`
/// (test-only cost), so the extra sort is free in every shipped build.
/// Used only there: every hidden-range producer in this crate is expected
/// to already emit disjoint ranges, so an overlap here means a producer bug
/// (the exact shape two separate findings on this branch turned out to be
/// — a fence's ranges colliding with its container's marker ranges) — this
/// makes it surface in tests instead of being silently absorbed by the
/// merge below.
fn has_overlap(intervals: &[(usize, usize)]) -> bool {
    let mut sorted: Vec<(usize, usize)> = intervals.to_vec();
    sorted.sort_by_key(|&(s, _)| s);
    sorted.windows(2).any(|w| match w {
        [(_, prev_end), (next_start, _)] => prev_end > next_start,
        _ => false,
    })
}

/// Merges overlapping or touching-adjacent intervals in `input` (any
/// order) into the minimal disjoint set covering the same bytes, sorted by
/// start. The chokepoint that makes "a byte is hidden at most once"
/// structural rather than every producer's responsibility: even if some
/// future producer reintroduces an overlapping-range bug (the class two
/// separate findings on this branch belonged to), the delta accumulation
/// below can no longer double-count it — merging happens UNCONDITIONALLY
/// in every build, so a producer bug degrades gracefully instead of
/// panicking; the `STRICT_INVARIANTS` assert above is what surfaces the
/// producer bug, in tests only. Sorting
/// lives HERE (not in each caller) so "unsorted intervals" can't
/// legitimately reach a caller that forgot to sort first — both current
/// callers (this module's own `build_line_conversions` and `rune-md`'s
/// `emit::unclaimed_subranges`, the visible-side counterpart of this same
/// collapse) used to sort before calling; that duplicated precondition is
/// gone now that the function enforces it itself. `pub`, not `pub(crate)`,
/// for that cross-crate reuse (WP3).
pub fn merge_overlapping(input: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    let mut sorted = input;
    sorted.sort_by_key(|&(s, _)| s);
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
/// (test-asserted, see `has_overlap` and this module's own `STRICT_INVARIANTS`)
/// coordinate inaccuracy rather than a doubly-counted delta corrupting
/// every position past it on the line.
pub(crate) fn build_line_conversions(
    starts: &[usize],
    hidden: &[Vec<(usize, usize)>],
) -> Vec<LineConversion> {
    let mut convs = Vec::with_capacity(hidden.len());
    for (line, ranges) in hidden.iter().enumerate() {
        let line_start = starts.get(line).copied().unwrap_or(0);
        let rel: Vec<(usize, usize)> = ranges
            .iter()
            .filter(|&&(s, e)| e > s)
            .map(|&(s, e)| (s.saturating_sub(line_start), e.saturating_sub(line_start)))
            .collect();

        // Never panics in an ordinary shipped build — only in tests
        // (or a build that opts in via the `strict-invariants` feature),
        // via the shared `assert_invariant` chokepoint. The merge two
        // lines below runs unconditionally regardless.
        assert_invariant(!has_overlap(&rel), || {
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
    /// `--release` test run too (the assert is test-only, not
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
