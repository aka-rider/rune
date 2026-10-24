use std::ops::Range;

use compact_str::{CompactString, ToCompactString};
use ratatui::style::Style;
use rune_core::{assert_invariant, codec};
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

// ratatui's cell_width() panics on a bare ASCII control byte in a
// single-byte symbol, so every control code is mapped to its Unicode
// "control picture" glyph before it reaches ratatui.
fn control_placeholder(ch: char) -> char {
    match ch as u32 {
        0x00..=0x1f => char::from_u32(0x2400 + ch as u32).unwrap_or('\u{FFFD}'),
        0x7f => '\u{2421}',
        _ => '\u{FFFD}',
    }
}

// A grapheme cluster (e.g. a ZWJ family emoji) must become exactly one
// Cell: ratatui's buffer diffing treats a cell whose cell_width() is > 1
// as covering the following cell_width()-1 columns and skips redrawing
// them, assuming a wide cell is never followed by non-blank content.
pub(crate) fn push_grapheme_cells(
    cells: &mut Vec<Cell>,
    visual_col: &mut usize,
    grapheme: &str,
    buf_offset: Option<u32>,
    style: Style,
) {
    push_grapheme_cells_with_raw(cells, visual_col, grapheme, buf_offset, style, style);
}

pub(crate) fn push_grapheme_cells_with_raw(
    cells: &mut Vec<Cell>,
    visual_col: &mut usize,
    grapheme: &str,
    buf_offset: Option<u32>,
    style: Style,
    raw_style: Style,
) {
    let mut chars = grapheme.chars();
    let Some(first) = chars.next() else {
        return;
    };
    if let Some(byte) = codec::raw_byte_value(first) {
        let width = control_aware_width(first);
        cells.push(Cell {
            text: codec::glyph(byte).into(),
            width: width as u8,
            style: raw_style,
            buf_offset,
        });
        *visual_col += width;
        let mut utf8 = [0u8; 4];
        for mark in chars {
            let mark = mark.encode_utf8(&mut utf8);
            push_grapheme_cells_with_raw(cells, visual_col, mark, buf_offset, style, raw_style);
        }
        return;
    }
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
                    style: raw_style,
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
    segment_cells_with(
        content,
        spans,
        |scope| style_for(theme, scope),
        theme.chrome.raw_byte,
    )
}

pub fn segment_geometry(content: &str, spans: &[SyntaxSpan]) -> Vec<Cell> {
    segment_cells_with(content, spans, |_| Style::default(), Style::default())
}

fn segment_cells_with(
    content: &str,
    spans: &[SyntaxSpan],
    style_of: impl Fn(ScopeId) -> Style,
    raw_overlay: Style,
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
        let raw_style = style.patch(raw_overlay);
        match sp {
            SyntaxSpan::Substituted { text, cell_map, .. } => {
                let mut char_idx = 0usize;
                for grapheme in text.graphemes(true) {
                    let offset = cell_map.get(char_idx).copied().flatten();
                    push_grapheme_cells_with_raw(
                        &mut cells,
                        &mut visual_col,
                        grapheme,
                        offset,
                        style,
                        raw_style,
                    );
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
                    push_grapheme_cells_with_raw(
                        &mut cells,
                        &mut visual_col,
                        grapheme,
                        cell_offset,
                        style,
                        raw_style,
                    );
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};
    use rune_syntax::scope::MarkdownScope;

    fn cells_of(theme: &Theme, content: &str) -> Vec<Cell> {
        let span = SyntaxSpan::identical(content, MarkdownScope::Text.into(), 0..content.len());
        segment_cells(theme, content, &[span])
    }

    fn shape(cells: &[Cell]) -> Vec<(&str, u8, Option<u32>)> {
        cells
            .iter()
            .map(|c| (c.text.as_str(), c.width, c.buf_offset))
            .collect()
    }

    #[test]
    fn a_raw_byte_renders_as_one_four_column_hex_cell_in_the_raw_byte_style() {
        let theme = Theme::catppuccin_mocha(false);
        let cells = cells_of(&theme, "a\u{10FFFF}b");
        assert_eq!(
            shape(&cells),
            [("a", 1, Some(0)), ("<ff>", 4, Some(1)), ("b", 1, Some(5)),]
        );
        assert_eq!(cells[1].style.fg, theme.chrome.raw_byte.fg);
        assert_ne!(cells[0].style.fg, theme.chrome.raw_byte.fg);
    }

    #[test]
    fn a_raw_byte_overrides_the_foreground_but_keeps_the_scope_background_and_weight() {
        let theme = Theme::catppuccin_mocha(false);
        let scope_style = Style::new()
            .fg(Color::Rgb(9, 9, 9))
            .bg(Color::Rgb(1, 2, 3))
            .bold();
        let span = SyntaxSpan::identical("\u{10FFFF}", MarkdownScope::Text.into(), 0..4);
        let cells = segment_cells_with(
            "\u{10FFFF}",
            &[span],
            |_| scope_style,
            theme.chrome.raw_byte,
        );
        assert_eq!(cells[0].style.fg, theme.chrome.raw_byte.fg);
        assert_eq!(cells[0].style.bg, Some(Color::Rgb(1, 2, 3)));
        assert!(cells[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_combining_mark_on_a_raw_byte_adds_a_zero_width_cell_and_no_column() {
        let theme = Theme::catppuccin_mocha(false);
        let cells = cells_of(&theme, "\u{10FF80}\u{0301}\u{0302}");
        let columns: usize = cells.iter().map(|c| usize::from(c.width)).sum();
        assert_eq!(cells[0].text, "<80>");
        assert_eq!(columns, 4);
        assert_eq!(columns, grapheme_width("\u{10FF80}\u{0301}\u{0302}"));
    }

    #[test]
    fn a_control_picture_placeholder_carries_the_raw_byte_style() {
        let theme = Theme::catppuccin_mocha(false);
        let cells = cells_of(&theme, "a\u{1}");
        assert_eq!(cells[1].text, "\u{2401}");
        assert_eq!(cells[1].style.fg, theme.chrome.raw_byte.fg);
    }

    #[test]
    fn geometry_widths_match_the_painted_widths_for_raw_bytes() {
        let theme = Theme::catppuccin_mocha(false);
        let content = "x\u{10FF9F}y";
        let span = SyntaxSpan::identical(content, MarkdownScope::Text.into(), 0..content.len());
        let painted: Vec<u8> = segment_cells(&theme, content, std::slice::from_ref(&span))
            .iter()
            .map(|c| c.width)
            .collect();
        let geometry: Vec<u8> = segment_geometry(content, &[span])
            .iter()
            .map(|c| c.width)
            .collect();
        assert_eq!(painted, geometry);
        assert_eq!(painted, [1, 4, 1]);
    }
}
