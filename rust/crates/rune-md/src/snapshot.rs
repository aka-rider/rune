//! `DisplaySnapshot` (plan Context, "Emit -> wrap -> snapshot"): in Phase 1
//! it is exactly the wrap rows, 1:1 — no table/image row expansion yet
//! (that's a Phase-5 concern). Kept as its own type, distinct from
//! `WrapSnapshot`, so Phase 5 can slot `ExpandTableRows`/`ExpandImageRows`
//! between wrap and display without changing this type's shape (plan:
//! "kept as a distinct type so Phase 5 slots `ExpandTableRows` between
//! them").

use crate::wrap::WrapSnapshot;

#[derive(Clone, Debug, Default)]
pub struct DisplaySnapshot {
    pub total_rows: usize,
}

impl DisplaySnapshot {
    /// Phase 1: identity over the wrap rows — no row expansion.
    pub fn from_wrap(wrap: &WrapSnapshot) -> DisplaySnapshot {
        DisplaySnapshot {
            total_rows: wrap.total_rows(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::SyntaxLine;
    use crate::wrap::WrapMap;

    #[test]
    fn display_snapshot_row_count_matches_wrap_in_phase_one() {
        // Two empty lines wrap to exactly 2 rows (each empty `SyntaxLine`
        // gets its own single, empty segment — see `WrapMap::wrap_line`'s
        // `line.spans.is_empty()` case). Pin that concrete count instead of
        // comparing `DisplaySnapshot::from_wrap`'s output back to
        // `wrap.total_rows()` — the very value it copies — which passes
        // trivially by construction regardless of what `total_rows()`
        // actually is.
        let lines = vec![SyntaxLine::default(), SyntaxLine::default()];
        let wrap = WrapMap::new(80).sync("", &lines);
        assert_eq!(wrap.total_rows(), 2);
        let display = DisplaySnapshot::from_wrap(&wrap);
        assert_eq!(display.total_rows, 2);
    }
}
