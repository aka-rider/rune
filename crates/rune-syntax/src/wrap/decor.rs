//! `WrapSegment`-level decoration (split out of the wrap
//! module, which is already over budget — this file carries the
//! attachment logic; the wrap module keeps only the call sites): turns a
//! `SyntaxLine`'s `LineDecor` (plan Context, "line decoration at the
//! display layer") into the per-segment `SegDecor` that rides alongside a
//! `WrapSegment`, resolving the two width questions the wrap pass owns —
//! how much of the line's own width budget the decor reserves before the
//! greedy breaker runs (`content_budget`), and what a GIVEN segment
//! actually renders once that decision is made (`attach`).
//!
//! A thematic-break rule (`LineDecor::is_rule`) is the one exception to
//! "decor that doesn't fit gets dropped": its width is chosen by the
//! emitter to exactly fill the line, so it never competes with content for
//! cells the way a heading icon or list bullet does — it always attaches,
//! clamped to whatever width is actually available instead.

use super::grapheme_width;
use crate::decor::LineDecor;
use crate::scope::ScopeId;
use unicode_segmentation::UnicodeSegmentation;

/// Which of a `DecorPiece`'s `first`/`cont` strings a segment resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentPosition {
    First,
    Continuation,
}

/// One rendered decoration piece, already resolved to whichever of a
/// `DecorPiece`'s `first`/`cont` strings applies to the segment it rides on,
/// and — for a rule line only — already clamped to the width that was
/// actually available.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegDecorPiece {
    pub text: String,
    pub scope: ScopeId,
}

/// A `WrapSegment`'s own decoration: sibling to `table`, never inside
/// `spans`, so the wrap query layer (`syntax_to_wrap`/`wrap_to_syntax`/
/// `visual_col`/`byte_col_from_visual`) stays byte-exact over span text
/// alone (module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SegDecor {
    pub pieces: Vec<SegDecorPiece>,
    /// Total display width across every piece's rendered `text`, in
    /// terminal cells — precomputed so a render-time consumer never has to
    /// re-measure through `grapheme_width` itself.
    pub cells: usize,
}

/// How many cells of the line's own width budget the greedy breaker should
/// leave to `line.decor` before laying out content — `0` when there is no
/// decor, or when the decor doesn't fit and (being non-rule) will be
/// dropped by `attach` below. A rule's decor never competes with content
/// for width (module docs: a thematic break has no visible spans, so this
/// only matters for the heading/list/quote producers, none of which set
/// `is_rule`), so it is intentionally excluded from this reservation.
pub fn content_budget(decor: Option<&LineDecor>, width: usize) -> usize {
    let Some(decor) = decor else {
        return width;
    };
    if decor.is_rule {
        return width;
    }
    let decor_cells = decor.cells();
    if decor_cells >= width {
        return width;
    }
    width.saturating_sub(decor_cells)
}

/// Resolve a line's decor into the `SegDecor` a given segment carries, or
/// `None` if there is no decor, or the decor doesn't fit and isn't the
/// rule exemption. `position` selects each piece's `first` string
/// (segment 0 of the line) versus its `cont` string (every later segment —
/// a wrapped continuation row).
pub fn attach(
    decor: Option<&LineDecor>,
    position: SegmentPosition,
    width: usize,
) -> Option<SegDecor> {
    let decor = decor?;
    if decor.pieces.is_empty() {
        return None;
    }

    if decor.is_rule {
        return Some(clamp_to_width(decor, position, width));
    }

    let decor_cells = decor.cells();
    if decor_cells >= width {
        return None;
    }

    let pieces = decor
        .pieces
        .iter()
        .map(|p| SegDecorPiece {
            text: match position {
                SegmentPosition::First => p.first.clone(),
                SegmentPosition::Continuation => p.cont.clone(),
            },
            scope: p.scope,
        })
        .collect();
    Some(SegDecor {
        pieces,
        cells: decor_cells,
    })
}

/// The rule-exemption path: renders every piece's chosen string, clamping
/// (never dropping) whatever doesn't fit in `width` cells — grapheme by
/// grapheme, so a clamp can never land mid-cluster.
fn clamp_to_width(decor: &LineDecor, position: SegmentPosition, width: usize) -> SegDecor {
    let mut remaining = width;
    let mut pieces = Vec::new();
    let mut total = 0usize;
    for piece in &decor.pieces {
        if remaining == 0 {
            break;
        }
        let raw = match position {
            SegmentPosition::First => &piece.first,
            SegmentPosition::Continuation => &piece.cont,
        };
        let (clamped, used) = clamp_text_to_cells(raw, remaining);
        if used > 0 {
            pieces.push(SegDecorPiece {
                text: clamped,
                scope: piece.scope,
            });
            total += used;
            remaining -= used;
        }
    }
    SegDecor {
        pieces,
        cells: total,
    }
}

/// Truncate `text` to at most `max_cells` of display width, breaking only
/// on whole grapheme-cluster boundaries (never mid-cluster) via the one
/// grapheme-width chokepoint. Returns the truncated text and its
/// actual rendered width, which may be less than `max_cells` when the next
/// cluster wouldn't fit.
fn clamp_text_to_cells(text: &str, max_cells: usize) -> (String, usize) {
    let mut out = String::new();
    let mut used = 0usize;
    for g in text.graphemes(true) {
        let w = grapheme_width(g);
        if used + w > max_cells {
            break;
        }
        out.push_str(g);
        used += w;
    }
    (out, used)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::decor::DecorPiece;
    use crate::scope::scope_table;

    fn scope() -> ScopeId {
        scope_table().resolve("markup.list").unwrap()
    }

    #[test]
    fn no_decor_reserves_no_budget_and_attaches_nothing() {
        assert_eq!(content_budget(None, 80), 80);
        assert_eq!(attach(None, SegmentPosition::First, 80), None);
    }

    #[test]
    fn decor_narrower_than_width_reserves_and_attaches() {
        let decor = LineDecor {
            pieces: vec![DecorPiece {
                first: "\u{2022} ".to_string(),
                cont: "  ".to_string(),
                scope: scope(),
            }],
            is_rule: false,
        };
        assert_eq!(content_budget(Some(&decor), 10), 8);
        let seg0 = attach(Some(&decor), SegmentPosition::First, 10).unwrap();
        assert_eq!(seg0.cells, 2);
        assert_eq!(seg0.pieces[0].text, "\u{2022} ");
        let cont = attach(Some(&decor), SegmentPosition::Continuation, 10).unwrap();
        assert_eq!(cont.pieces[0].text, "  ");
    }

    #[test]
    fn decor_at_or_wider_than_width_is_dropped_when_not_a_rule() {
        let decor = LineDecor {
            pieces: vec![DecorPiece {
                first: "1234567890".to_string(),
                cont: "          ".to_string(),
                scope: scope(),
            }],
            is_rule: false,
        };
        assert_eq!(content_budget(Some(&decor), 10), 10);
        assert_eq!(attach(Some(&decor), SegmentPosition::First, 10), None);
    }

    #[test]
    fn rule_decor_always_attaches_clamped_to_width() {
        let decor = LineDecor {
            pieces: vec![DecorPiece {
                first: "\u{2500}".repeat(20),
                cont: String::new(),
                scope: scope(),
            }],
            is_rule: true,
        };
        // A rule never reserves content budget (it has no competing spans).
        assert_eq!(content_budget(Some(&decor), 10), 10);
        let seg = attach(Some(&decor), SegmentPosition::First, 10).unwrap();
        assert_eq!(seg.cells, 10);
        assert_eq!(seg.pieces[0].text.chars().count(), 10);
    }

    #[test]
    fn rule_decor_degrades_to_zero_cells_at_width_zero_without_panicking() {
        let decor = LineDecor {
            pieces: vec![DecorPiece {
                first: "\u{2500}".repeat(5),
                cont: String::new(),
                scope: scope(),
            }],
            is_rule: true,
        };
        let seg = attach(Some(&decor), SegmentPosition::First, 0).unwrap();
        assert_eq!(seg.cells, 0);
        assert!(seg.pieces.is_empty());
    }

    #[test]
    fn clamp_to_width_drops_a_piece_that_cannot_fit_even_one_grapheme() {
        // The second piece is a single double-cell grapheme with only one
        // cell left after the first piece — it must be omitted entirely,
        // not pushed as an empty-text piece.
        let decor = LineDecor {
            pieces: vec![
                DecorPiece {
                    first: "\u{2500}\u{2500}".to_string(),
                    cont: String::new(),
                    scope: scope(),
                },
                DecorPiece {
                    first: "\u{4e2d}".to_string(),
                    cont: String::new(),
                    scope: scope(),
                },
            ],
            is_rule: true,
        };
        let seg = attach(Some(&decor), SegmentPosition::First, 3).unwrap();
        assert_eq!(
            seg.pieces.len(),
            1,
            "a piece with no room for even one grapheme must be dropped, not pushed empty"
        );
        assert_eq!(seg.cells, 2);
    }

    #[test]
    fn clamp_to_width_shrinks_the_remaining_budget_by_what_the_previous_piece_used() {
        // Width 5, a 3-cell first piece and a 5-cell second piece: only 2
        // cells remain for the second piece once the first is placed, so the
        // total must land on exactly 5 — never less (budget not shrunk) nor
        // more (budget shrunk by the wrong amount or grown instead).
        let decor = LineDecor {
            pieces: vec![
                DecorPiece {
                    first: "\u{2500}".repeat(3),
                    cont: String::new(),
                    scope: scope(),
                },
                DecorPiece {
                    first: "\u{2500}".repeat(5),
                    cont: String::new(),
                    scope: scope(),
                },
            ],
            is_rule: true,
        };
        let seg = attach(Some(&decor), SegmentPosition::First, 5).unwrap();
        assert_eq!(
            seg.cells, 5,
            "total rendered decor width must never exceed the line's own width"
        );
        let total_chars: usize = seg.pieces.iter().map(|p| p.text.chars().count()).sum();
        assert_eq!(total_chars, 5);
    }
}
