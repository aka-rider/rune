use crate::ScopeId;
use crate::wrap::grapheme_width;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecorPiece {
    pub first: String,
    pub cont: String,
    pub scope: ScopeId,
}

impl DecorPiece {
    pub fn cells(&self) -> usize {
        self.first.graphemes(true).map(grapheme_width).sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineDecor {
    pub pieces: Vec<DecorPiece>,
    pub is_rule: bool,
}

impl LineDecor {
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
                    first: "\u{2022} ".to_string(),
                    cont: "  ".to_string(),
                    scope,
                },
                DecorPiece {
                    first: "\u{258E}".to_string(),
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
