use std::ops::Range;

use compact_str::{CompactString, ToCompactString};
use ratatui::style::Style;
use rune_core::assert_invariant;
use unicode_segmentation::UnicodeSegmentation;

use rune_syntax::ScopeId;
use rune_syntax::SyntaxSpan;
use rune_syntax::wrap::{control_aware_width, grapheme_width, rune_width_with_tab};

use crate::theme::Theme;

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: CompactString,
    pub width: u8,
    pub style: Style,
    pub buf_offset: Option<u32>,
}

pub fn style_for(theme: &Theme, id: ScopeId) -> Style {
    theme.scope_style(id)
}

// ratatui-core's cell_width() asserts a single-byte symbol is never an
// ASCII control character; a raw control byte reaching set_symbol panics a
// debug build and corrupts the row in release, so every control code maps
// to its Unicode "control picture" glyph before it ever gets there.
fn control_placeholder(ch: char) -> char {
    match ch as u32 {
        0x00..=0x1f => char::from_u32(0x2400 + ch as u32).unwrap_or('\u{FFFD}'),
        0x7f => '\u{2421}',
        _ => '\u{FFFD}',
    }
}

// A ZWJ sequence (e.g. a family emoji) or a skin-tone-modified emoji is many
// chars forming one user-perceived grapheme cluster. Splitting it across
// multiple Cells corrupts the display: ratatui's own buffer diffing treats
// any cell whose cell_width() is > 1 as covering the next cell_width()-1
// columns too and skips redrawing them, assuming no double-width cell is
// ever followed by non-blank content — so a cluster must be emitted as one
// Cell, and `blit` resets the buffer columns a wide Cell covers to keep
// that assumption true for cells written directly via cell_mut.
pub(crate) fn push_grapheme_cells(
    cells: &mut Vec<Cell>,
    visual_col: &mut usize,
    grapheme: &str,
    buf_offset: Option<u32>,
    style: Style,
) {
    let mut chars = grapheme.chars();
    let Some(first) = chars.next() else {
        return;
    };
    if chars.next().is_none() {
        match first {
            '\n' | '\r' => return,
            '\t' => {
                let width = rune_width_with_tab(first, *visual_col);
                for _ in 0..width {
                    cells.push(Cell {
                        text: " ".into(),
                        width: 1,
                        style,
                        buf_offset,
                    });
                }
                *visual_col += width;
                return;
            }
            _ if first.is_control() => {
                let width = control_aware_width(first);
                cells.push(Cell {
                    text: control_placeholder(first).to_compact_string(),
                    width: width as u8,
                    style,
                    buf_offset,
                });
                *visual_col += width;
                return;
            }
            _ => {}
        }
    }

    let width = grapheme_width(grapheme);
    cells.push(Cell {
        text: grapheme.into(),
        width: width as u8,
        style,
        buf_offset,
    });
    *visual_col += width;
}

pub fn segment_cells(theme: &Theme, content: &str, spans: &[SyntaxSpan]) -> Vec<Cell> {
    segment_cells_with(content, spans, |scope| style_for(theme, scope))
}

pub fn segment_geometry(content: &str, spans: &[SyntaxSpan]) -> Vec<Cell> {
    segment_cells_with(content, spans, |_| Style::default())
}

fn segment_cells_with(
    content: &str,
    spans: &[SyntaxSpan],
    style_of: impl Fn(ScopeId) -> Style,
) -> Vec<Cell> {
    let capacity: usize = spans
        .iter()
        .map(|sp| match sp {
            SyntaxSpan::Substituted { text, .. } => text.len(),
            SyntaxSpan::Identical { .. } => sp.range().len(),
        })
        .sum();
    let mut cells = Vec::with_capacity(capacity);
    let mut visual_col = 0usize;
    for sp in spans {
        let style = style_of(sp.scope());
        match sp {
            SyntaxSpan::Substituted { text, cell_map, .. } => {
                let mut char_idx = 0usize;
                for grapheme in text.graphemes(true) {
                    let offset = cell_map.get(char_idx).copied().flatten();
                    push_grapheme_cells(&mut cells, &mut visual_col, grapheme, offset, style);
                    char_idx += grapheme.chars().count();
                }
            }
            SyntaxSpan::Identical { .. } => {
                let mut offset = sp.range().start;
                for grapheme in sp.text(content).graphemes(true) {
                    let cell_offset = u32::try_from(offset).ok();
                    assert_invariant!(cell_offset.is_some(), || format!(
                        "span byte offset {offset} exceeds the cell offset range"
                    ));
                    push_grapheme_cells(&mut cells, &mut visual_col, grapheme, cell_offset, style);
                    offset += grapheme.len();
                }
            }
        }
    }
    cells
}

pub(crate) fn paint_range(rows: &mut [Vec<Cell>], range: Range<usize>, style: Style) {
    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            let Some(offset) = cell.buf_offset else {
                continue;
            };
            if range.contains(&(offset as usize)) {
                cell.style = cell.style.patch(style);
            }
        }
    }
}
