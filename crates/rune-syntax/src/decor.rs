//! Line decoration — glyphs a producer wants painted at the START of a
//! rendered line (heading icons, list bullets/numbers, blockquote bars,
//! thematic-break rules) that do NOT correspond to any buffer byte. Kept
//! out-of-band from `SyntaxSpan`/`SyntaxLine::spans` on purpose (plan
//! Context: "line decoration at the display layer"): a `Substituted` span's
//! text must stay byte-length-neutral with the buffer range it replaces, and
//! several of these glyphs (nerd-font private-use-area codepoints, multi-cell
//! bullets) cannot honor that constraint. `LineDecor` instead rides
//! alongside a `SyntaxLine`, then a `WrapSegment`, then a `DisplayRow`, and
//! is finally prefixed as cells with no buffer position at render time — the
//! same `-1`-sentinel convention table chrome already uses.

use crate::wrap::grapheme_width;
use crate::ScopeId;
use unicode_segmentation::UnicodeSegmentation;

/// One decorative glyph run. `first` is what a line's FIRST visual row
/// shows; `cont` is what every WRAPPED CONTINUATION row of the same source
/// line shows instead (usually blank padding of equal width, except a
/// blockquote bar, which repeats on every continuation row too). The two
/// strings are equal-width by construction — callers building a
/// `DecorPiece` are responsible for keeping them that way; nothing here
/// enforces it structurally, but every producer in `rune-md` measures both
/// through the same `cells()` helper before pairing them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecorPiece {
    pub first: String,
    pub cont: String,
    pub scope: ScopeId,
}

impl DecorPiece {
    /// Display width of `first` in terminal cells, via the one grapheme-width
    /// chokepoint (§1.5) — never a byte or `char` count.
    pub fn cells(&self) -> usize {
        self.first.graphemes(true).map(grapheme_width).sum()
    }
}

/// A line's full decoration: one or more pieces (nested list/quote markers
/// contribute one piece each, outermost first) plus whether this decoration
/// is a thematic-break rule — the one decor kind exempt from the
/// wrap layer's "drop decor that doesn't fit" rule (WP3.S3), since a rule's
/// width is chosen to exactly fill the line rather than compete with content
/// for cells.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineDecor {
    pub pieces: Vec<DecorPiece>,
    pub is_rule: bool,
}

impl LineDecor {
    /// Total display width across every piece's `first` string, in terminal
    /// cells — what the wrap layer must reserve before laying out the line's
    /// own content.
    pub fn cells(&self) -> usize {
        self.pieces.iter().map(DecorPiece::cells).sum()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::scope::scope_table;

    #[test]
    fn cells_sums_piece_widths_via_grapheme_width() {
        let scope = scope_table().resolve("markup.list").unwrap();
        let decor = LineDecor {
            pieces: vec![
                DecorPiece {
                    first: "\u{2022} ".to_string(), // bullet + space: 2 cells
                    cont: "  ".to_string(),
                    scope,
                },
                DecorPiece {
                    first: "\u{258E}".to_string(), // quote bar: 1 cell
                    cont: "\u{258E}".to_string(),
                    scope,
                },
            ],
            is_rule: false,
        };
        assert_eq!(decor.cells(), 3);
    }

    #[test]
    fn empty_decor_has_zero_cells() {
        assert_eq!(LineDecor::default().cells(), 0);
    }
}
