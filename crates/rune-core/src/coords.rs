//! Distinct coordinate-space types — Buffer / Syntax / Wrap / Display — so a
//! conversion between spaces can never be silently skipped by mixing up a
//! plain `usize`. Each offset is a distinct newtype (a single-field tuple
//! struct) — a real distinct type, not merely an alias.

/// Buffer Space — raw byte positions in the UTF-8 document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferOffset(pub usize);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferPoint {
    /// 0-indexed model line number.
    pub line: usize,
    /// Byte offset from the start of that line.
    pub col: usize,
}

/// A terminal-CELL column (display width is CELLS via `unicode-width`
/// over grapheme clusters, never bytes) — distinct from a `BufferPoint.col`
/// (a BYTE column) so a value measured in one unit can never be replayed as
/// the other by mixing up a plain `usize`. Exists because a cell count
/// measured on one line is not portable to a different line's bytes without
/// a `next_grapheme`/`byte_col_from_visual`-style walk first (a different
/// line can hold different-width characters at the same cell column).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VisualCol(pub usize);

/// Syntax Space — positions after markdown tokens are folded/expanded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyntaxOffset(pub usize);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyntaxPoint {
    /// Same model line as buffer (1:1 line mapping).
    pub line: usize,
    /// Column in syntax space.
    pub col: usize,
}

/// Wrap Space — positions after soft-wrap breaks are inserted. Frozen
/// before table/image row expansion runs; NOT the same space as Display
/// (below).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WrapRow(pub usize);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WrapPoint {
    /// Row in wrap-space (0-indexed from doc top), pre table/image expansion.
    pub row: usize,
    /// Column within that wrapped segment.
    pub col: usize,
}

/// Display Space — final terminal grid after table/image row expansion AND
/// viewport slicing. A Display row can exceed the Wrap row count once
/// expansion runs; converting a Display row to a Wrap row is NOT direct
/// arithmetic (Phase 5 concern — out of scope here).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayRow(pub usize);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayPoint {
    /// Row relative to viewport top, in POST-expansion display-space.
    pub row: usize,
    /// Column (includes tab expansion).
    pub col: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_types_hold_their_fields() {
        let bo = BufferOffset(10);
        let bp = BufferPoint { line: 1, col: 5 };
        assert_eq!(bo, BufferOffset(10));
        assert_eq!(bp.line, 1);
        assert_eq!(bp.col, 5);

        let vc = VisualCol(7);
        assert_eq!(vc, VisualCol(7));

        let so = SyntaxOffset(20);
        let sp = SyntaxPoint { line: 2, col: 10 };
        assert_eq!(so, SyntaxOffset(20));
        assert_eq!(sp.line, 2);
        assert_eq!(sp.col, 10);

        let wr = WrapRow(30);
        let wp = WrapPoint { row: 3, col: 15 };
        assert_eq!(wr, WrapRow(30));
        assert_eq!(wp.row, 3);
        assert_eq!(wp.col, 15);

        let dr = DisplayRow(40);
        let dp = DisplayPoint { row: 4, col: 20 };
        assert_eq!(dr, DisplayRow(40));
        assert_eq!(dp.row, 4);
        assert_eq!(dp.col, 20);
    }
}
