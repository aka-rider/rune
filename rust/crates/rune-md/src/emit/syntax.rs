//! `SyntaxSpan`/`SyntaxLine` (the Emitter's per-buffer-line output) and
//! `SyntaxSnapshot` (Buffer Space <-> Syntax Space coordinate conversion) —
//! port of `pkg/editor/display/syntax_snapshot.go:35-97`.

use crate::element::RevealState;
use crate::emit::style::StyleId;
use rune_core::coords::{BufferPoint, SyntaxPoint};

/// Per-visual-char buffer offset, `-1` for decorative/padding cells with no
/// buffer correspondence — port of `pkg/editor/display/cellmap.go`'s
/// `CellMapping`. Phase 1 never produces `-1` (no decorative padding in this
/// crate yet); the type still carries it so the proptest invariant
/// ("entries are -1 or valid char boundaries") is meaningful, and so a
/// future decorative producer doesn't need a type change.
pub type CellMap = Vec<i64>;

#[derive(Clone, Debug)]
pub struct SyntaxSpan {
    pub text: String,
    pub style: StyleId,
    pub state: RevealState,
    pub buffer_start: usize,
    pub buffer_end: usize,
    /// Only `Some` for `Rendered` spans (plan: "`cell_map` only for
    /// Rendered spans, one buffer offset per char").
    pub cell_map: Option<CellMap>,
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

/// Builds the per-line `LineConversion` table from each line's raw hidden
/// byte ranges (absolute buffer offsets) — the accumulating-delta model
/// `SyntaxSnapshot::buffer_to_syntax`/`syntax_to_buffer` read.
pub(crate) fn build_line_conversions(
    starts: &[usize],
    hidden: &[Vec<(usize, usize)>],
) -> Vec<LineConversion> {
    let mut convs = Vec::with_capacity(hidden.len());
    for (line, ranges) in hidden.iter().enumerate() {
        let line_start = starts.get(line).copied().unwrap_or(0);
        let mut rel: Vec<(usize, usize)> = ranges
            .iter()
            .map(|&(s, e)| (s.saturating_sub(line_start), e.saturating_sub(line_start)))
            .collect();
        rel.sort_by_key(|&(s, _)| s);

        let mut deltas = Vec::with_capacity(rel.len());
        let mut hidden_ranges = Vec::with_capacity(rel.len());
        let mut accum = 0usize;
        for (s, e) in rel {
            if e <= s {
                continue;
            }
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
