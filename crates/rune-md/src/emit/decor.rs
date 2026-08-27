use super::EmitOut;
use super::style::{heading_style, list_marker_style, quote_marker_scope};
use rune_syntax::ScopeId;
use rune_syntax::wrap::grapheme_width;
use rune_syntax::{DecorPiece, LineDecor};
use unicode_segmentation::UnicodeSegmentation;

fn push_piece(out: &mut EmitOut, line: usize, piece: DecorPiece) {
    let Some(slot) = out.decors.get_mut(line) else {
        return;
    };
    slot.get_or_insert_with(LineDecor::default)
        .pieces
        .push(piece);
}

fn blank_cont(first: &str) -> String {
    let cells: usize = first.graphemes(true).map(grapheme_width).sum();
    " ".repeat(cells)
}

pub(crate) fn push_heading_decor(out: &mut EmitOut, line: usize, level: u8) {
    let idx = (level.saturating_sub(1) as usize).min(out.icons.headings.len() - 1);
    let Some(&glyph) = out.icons.headings.get(idx) else {
        return;
    };
    let piece = DecorPiece {
        first: glyph.to_string(),
        cont: blank_cont(glyph),
        scope: heading_style(level),
    };
    push_piece(out, line, piece);
}

pub(crate) fn push_list_marker_decor(
    out: &mut EmitOut,
    line: usize,
    ordered: bool,
    depth: u8,
    marker_text: &str,
) {
    let first = if ordered {
        let trimmed = marker_text.trim_end();
        format!("{trimmed} ")
    } else {
        let idx = (depth as usize) % out.icons.bullets.len();
        let bullet = out.icons.bullets.get(idx).copied().unwrap_or("\u{2022}");
        format!("{bullet} ")
    };
    let piece = DecorPiece {
        cont: blank_cont(&first),
        first,
        scope: list_marker_style(false),
    };
    push_piece(out, line, piece);
}

pub(crate) fn push_quote_marker_decor(out: &mut EmitOut, line: usize) {
    let bar = out.icons.quote_bar.to_string();
    let piece = DecorPiece {
        first: bar.clone(),
        cont: bar,
        scope: quote_marker_scope(),
    };
    push_piece(out, line, piece);
}

fn push_rule_decor(out: &mut EmitOut, line: usize, scope: ScopeId) {
    let rule_cells = grapheme_width(out.icons.rule).max(1);
    let count = (out.width as usize) / rule_cells;
    let piece = DecorPiece {
        first: out.icons.rule.repeat(count),
        cont: String::new(),
        scope,
    };
    push_piece(out, line, piece);
    if let Some(Some(decor)) = out.decors.get_mut(line) {
        decor.is_rule = true;
    }
}

pub(crate) fn push_hr_decor(out: &mut EmitOut, line: usize) {
    push_rule_decor(out, line, super::style::hr_scope());
}

pub(crate) fn push_heading_rule_decor(out: &mut EmitOut, line: usize, level: u8) {
    push_rule_decor(out, line, heading_style(level));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_cont_is_one_space_per_cell_of_the_first_piece() {
        assert_eq!(blank_cont("ab"), "  ");
        assert_eq!(blank_cont(""), "");
    }
}
