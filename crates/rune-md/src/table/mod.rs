//! Table rendering (WP2.S5 onward): span-tiling, cell rendering, and Grid/
//! Wrapped/Pivoted layout for GFM tables.
//!
//! `row_spans` is the tiling chokepoint every table row (Grid today;
//! Wrapped/Pivoted extra rows later) funnels through before its output ever
//! reaches `SyntaxLine::spans`. The reason it has to be exact: a table row
//! substitutes its ENTIRE rendered text for the source line's raw
//! `| a | b |`, but `fill_gaps` (`emit::mod`) still runs over every line
//! afterward — any byte of the source line left unclaimed in `accounted`
//! comes back as a spurious `Identical` span carrying literal markdown
//! text, spliced into the middle of the rendered row. So row 1's spans
//! (this function's own output) MUST tile `[line_start, line_start +
//! line_len)` exactly, with no gap and no overlap, or that safety net
//! corrupts the render instead of protecting it.

pub mod layout;
pub mod pivot;
pub mod render;
pub mod wrapped;

use rune_syntax::{CellMap, ScopeId, SyntaxSpan};

/// One visible char's provenance inside a table's rendered row: the
/// absolute buffer offset it maps back to, or `-1` for a decorative char
/// with no buffer correspondence at all (a `│` border, a padding space, a
/// pivot label borrowed from a different line — see `table::layout`'s
/// docs). `scope` is carried alongside so a caller building a flat
/// char-by-char sequence (`layout::grid_row`/`separator_row`) can group
/// contiguous same-scope runs before handing them to `row_spans` — `scope`
/// itself is never read by `row_spans`, which only ever consumes a run's
/// OWN uniform `ScopeId` (the third element of its `runs` tuples) and each
/// char's `buf` (to build `cell_map`).
#[derive(Clone, Copy, Debug)]
pub struct CellSrc {
    pub buf: i64,
    pub scope: ScopeId,
}

/// The first non-negative `buf` in `run`, if any.
fn anchor(run: &[CellSrc]) -> Option<usize> {
    run.iter().find(|c| c.buf >= 0).map(|c| c.buf as usize)
}

/// The last non-negative `buf` in `run` plus that char's UTF-8 byte length —
/// i.e. the buffer offset one past the run's own last real (non-decorative)
/// char — if any. `text` and `run` are the same run's chars/provenance,
/// zipped by position (both always the same length, `render_cell`'s own
/// invariant).
fn content_end(text: &str, run: &[CellSrc]) -> Option<usize> {
    let mut end = None;
    for (ch, src) in text.chars().zip(run.iter()) {
        if src.buf >= 0 {
            end = Some(src.buf as usize + ch.len_utf8());
        }
    }
    end
}

/// Tiles a table row's runs across `[line_start, line_start + line_len)`
/// exactly — see this module's docs for why that's load-bearing, not just
/// tidy. `runs` is `(visible text, one CellSrc per char, the run's own
/// scope)`, already grouped by a caller (`layout::grid_row`/
/// `separator_row`) into maximal same-scope pieces; a run with no anchor at
/// all (fully decorative: a `│` border, a padding run) borrows its start
/// from the previous run's content end, or — if THAT has none either — the
/// previous run's own start, collapsing to a zero-length range rather than
/// inventing a position. Ported literally from the plan's worked example
/// (WP2.S5): `starts[0] = line_start`; for `i > 0`, `starts[i] = anchor(i)`,
/// else `content_end(i-1)`, else `starts[i-1]`, then clamped to never go
/// backwards; `ends[i] = starts[i+1]` for all but the last run, whose end
/// is pinned to `line_start + line_len`.
pub fn row_spans(
    line_start: usize,
    line_len: usize,
    runs: &[(String, Vec<CellSrc>, ScopeId)],
) -> Vec<SyntaxSpan> {
    let n = runs.len();
    if n == 0 {
        return Vec::new();
    }
    let line_end = line_start + line_len;

    // `starts[0] = line_start`; for `i > 0`, `starts[i] = anchor(i)`, else
    // `content_end(i-1)`, else the previous run's own start — built by
    // pushing (never indexing: `.get`/`.last` throughout, the workspace
    // denies `clippy::indexing_slicing` in production code).
    let mut starts: Vec<usize> = Vec::with_capacity(n);
    for (i, (_, run, _)) in runs.iter().enumerate() {
        let prev_start = starts.last().copied().unwrap_or(line_start);
        let candidate = if i == 0 {
            line_start
        } else {
            let prev = runs.get(i - 1).map(|(t, r, _)| (t.as_str(), r.as_slice()));
            anchor(run)
                .or_else(|| prev.and_then(|(t, r)| content_end(t, r)))
                .unwrap_or(prev_start)
        };
        starts.push(candidate.max(prev_start));
    }

    // `ends[i] = starts[i+1]` for every run but the last, whose end is
    // pinned to `line_end` — expressed as a zip against `starts` shifted by
    // one rather than an index-plus-one lookup.
    let mut ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .copied()
        .chain(std::iter::once(line_end))
        .zip(starts.iter())
        .map(|(e, &s)| e.max(s))
        .collect();
    // `zip` above already stops at `starts.len()`, so `ends.len() ==
    // starts.len() == n` by construction; the explicit resize is a no-op
    // safety net, not load-bearing.
    ends.resize(n, line_end);

    let mut spans = Vec::with_capacity(n);
    for (i, (text, run, scope)) in runs.iter().enumerate() {
        let s = starts.get(i).copied().unwrap_or(line_start);
        let e = ends.get(i).copied().unwrap_or(s);
        let cell_map: CellMap = run.iter().map(|c| c.buf).collect();
        debug_assert_eq!(
            cell_map.len(),
            text.chars().count(),
            "row_spans: cell_map length must equal the run's own visible char count"
        );
        spans.push(SyntaxSpan::Substituted {
            scope: *scope,
            text: text.clone(),
            range: s..e,
            cell_map,
        });
    }

    debug_assert!(
        spans.first().is_none_or(|s| s.range().start == line_start),
        "row_spans: first span must start exactly at line_start"
    );
    debug_assert!(
        spans.last().is_none_or(|s| s.range().end == line_end),
        "row_spans: last span must end exactly at line_start + line_len"
    );
    debug_assert!(
        spans.windows(2).all(|w| match (w.first(), w.get(1)) {
            (Some(a), Some(b)) => a.range().end == b.range().start,
            _ => true,
        }),
        "row_spans: span ranges must tile contiguously with no gap or overlap"
    );

    spans
}

/// Builds the `SyntaxSpan`s for a Wrapped/Pivoted table row's CONTINUATION
/// visual row (row 2..N of one source line — `TableRowInfo::extra_rows`,
/// WP4). Unlike `row_spans`, this does NOT tile `[line_start, line_start +
/// line_len)`: an extra row claims no bytes at all (Gotcha 2). Every span
/// gets the SAME empty `range` (`line_start..line_start`), so it never
/// enters `accounted`/`fill_gaps`'s bookkeeping — that machinery only ever
/// sees a line's OWN `spans`, never its `extra_rows`. One `SyntaxSpan` per
/// run, preserving each run's own scope and per-char `cell_map` — only the
/// `range` is synthetic.
pub fn extra_row_spans(
    line_start: usize,
    runs: &[(String, Vec<CellSrc>, ScopeId)],
) -> Vec<SyntaxSpan> {
    runs.iter()
        .map(|(text, run, scope)| {
            let cell_map: CellMap = run.iter().map(|c| c.buf).collect();
            debug_assert_eq!(
                cell_map.len(),
                text.chars().count(),
                "extra_row_spans: cell_map length must equal the run's own visible char count"
            );
            SyntaxSpan::Substituted {
                scope: *scope,
                text: text.clone(),
                range: line_start..line_start,
                cell_map,
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn src(buf: i64) -> CellSrc {
        CellSrc {
            buf,
            scope: ScopeId(0),
        }
    }

    /// The plan's own worked example (WP2.S5): `"| a | bb |"`, column
    /// widths 2/2, tiled from runs `[│␠, a, ␠, ␠│␠, bb, ␠│]` (decorative
    /// runs carry `buf = -1` for every char). Buffer offsets: `'a'` sits at
    /// 2, `'bb'` at 6-7 (0-indexed into the 10-byte line).
    #[test]
    fn worked_example_tiles_exactly_as_the_plan_specifies() {
        let runs: Vec<(String, Vec<CellSrc>, ScopeId)> = vec![
            ("| ".to_string(), vec![src(-1), src(-1)], ScopeId(1)),
            ("a".to_string(), vec![src(2)], ScopeId(2)),
            (" ".to_string(), vec![src(-1)], ScopeId(2)),
            (
                " | ".to_string(),
                vec![src(-1), src(-1), src(-1)],
                ScopeId(1),
            ),
            ("bb".to_string(), vec![src(6), src(7)], ScopeId(2)),
            (" |".to_string(), vec![src(-1), src(-1)], ScopeId(1)),
        ];
        let spans = row_spans(0, 10, &runs);
        let ranges: Vec<(usize, usize)> = spans
            .iter()
            .map(|s| (s.range().start, s.range().end))
            .collect();
        assert_eq!(
            ranges,
            vec![(0, 2), (2, 3), (3, 3), (3, 6), (6, 8), (8, 10)]
        );
    }

    #[test]
    fn fully_decorative_row_tiles_into_one_span_covering_the_whole_line() {
        let runs: Vec<(String, Vec<CellSrc>, ScopeId)> =
            vec![("├───┤".to_string(), vec![src(-1); 5], ScopeId(3))];
        let spans = row_spans(100, 5, &runs);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].range(), 100..105);
    }

    /// Gotcha 2: an extra row's spans claim NO bytes — every one gets the
    /// SAME empty range at the line's own start, regardless of how many
    /// runs it has or what real buffer offsets its own `cell_map` carries.
    #[test]
    fn extra_row_spans_all_carry_the_same_empty_range_at_line_start() {
        let runs: Vec<(String, Vec<CellSrc>, ScopeId)> = vec![
            ("  Name: ".to_string(), vec![src(-1); 8], ScopeId(1)),
            (
                "Alice".to_string(),
                vec![src(10), src(11), src(12), src(13), src(14)],
                ScopeId(2),
            ),
        ];
        let spans = extra_row_spans(50, &runs);
        assert_eq!(spans.len(), 2);
        for s in &spans {
            assert_eq!(s.range(), 50..50);
        }
        // The real buf offsets are preserved in each span's own cell_map
        // even though `range` itself carries none of them.
        let cell_map = match &spans[1] {
            SyntaxSpan::Substituted { cell_map, .. } => cell_map.clone(),
            SyntaxSpan::Identical { .. } => Vec::new(),
        };
        assert_eq!(cell_map, vec![10, 11, 12, 13, 14]);
    }
}
