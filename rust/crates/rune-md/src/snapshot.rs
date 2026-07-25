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
        let lines = vec![SyntaxLine::default(), SyntaxLine::default()];
        let wrap = WrapMap::new(80).sync(&lines);
        let display = DisplaySnapshot::from_wrap(&wrap);
        assert_eq!(display.total_rows, wrap.total_rows());
    }
}
