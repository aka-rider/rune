//! Line-decoration producers (plan WP2.S5): builds the `LineDecor` a
//! Rendered heading/list-item/blockquote-marker/thematic-break line
//! carries, appended into `EmitOut::decors`. Table lines are never
//! decorated — `EmitOut::decors` simply stays `None` there, since
//! `emit_table` (sibling module) never calls into this one.
//!
//! Every producer measures cell width through `DecorPiece::cells`, which
//! routes to the one `rune_syntax::wrap::grapheme_width` chokepoint (§1.5)
//! — never a raw `.len()` byte count or `.chars().count()`.

use super::EmitOut;
use super::style::{heading_style, list_marker_style, quote_marker_scope};
use rune_syntax::ScopeId;
use rune_syntax::wrap::grapheme_width;
use rune_syntax::{DecorPiece, LineDecor};
use unicode_segmentation::UnicodeSegmentation;

/// Appends `piece` to `line`'s decor, creating an empty `LineDecor` first on
/// this line's first piece. Nested containers (a doubly-nested blockquote
/// marker) each contribute their OWN piece to the SAME line, outermost
/// first — the walk visits a container's own marker before recursing into
/// its children, so call order here already matches nesting order.
fn push_piece(out: &mut EmitOut, line: usize, piece: DecorPiece) {
    let Some(slot) = out.decors.get_mut(line) else {
        return;
    };
    slot.get_or_insert_with(LineDecor::default)
        .pieces
        .push(piece);
}

/// A blank-padding string of the same cell width as `first` — every
/// continuation row of a wrapped decorated line gets this instead (WP3
/// reads it), except the blockquote bar, which repeats `first` itself
/// (see `push_quote_marker_decor`).
fn blank_cont(first: &str) -> String {
    let cells: usize = first.graphemes(true).map(grapheme_width).sum();
    " ".repeat(cells)
}

/// Heading icon (plan A1/A2, `IconSet::headings`): one piece, level 1..6
/// indexes `headings[0..=5]`, clamped so an out-of-range level (never
/// produced by a real parse, but not structurally impossible) degrades to
/// the H6 glyph instead of panicking (§1.3).
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

/// A list item's own marker decor (plan WP2.S5): unordered items get a
/// bullet chosen by nesting `depth` (cycling `IconSet::bullets`); ordered
/// items keep the user's own marker verbatim, delimiter included —
/// `marker_text` trimmed of trailing whitespace and re-suffixed with a
/// single space, so `"1."` renders as `"1. "` and `"12)"` as `"12) "`,
/// never renumbered and never re-delimited.
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

/// One blockquote marker's bar (plan WP2.S5): `cont` repeats `first` — the
/// one decor kind where a wrapped continuation row still shows the marker,
/// matching how a real blockquote's `"> "` prefix repeats on every wrapped
/// line of its own content.
pub(crate) fn push_quote_marker_decor(out: &mut EmitOut, line: usize) {
    let bar = out.icons.quote_bar.to_string();
    let piece = DecorPiece {
        first: bar.clone(),
        cont: bar,
        scope: quote_marker_scope(),
    };
    push_piece(out, line, piece);
}

/// A full-width rule, `scope`-parameterized: repeats `IconSet::rule` to fill
/// `EmitOut::width` cells exactly, `is_rule: true` so WP3's wrap layer
/// exempts it from the width-drop rule (a rule competes with no content for
/// cells, unlike every other decor kind). `cont` is empty — every caller of
/// this producer is always exactly one line, it never wraps. The one shared
/// chokepoint behind both a thematic break's rule (`hr_scope()`) and a
/// setext heading's underline row (`heading_style(level)`) — same shape,
/// different scope.
fn push_rule_decor(out: &mut EmitOut, line: usize, scope: ScopeId) {
    let rule_cells = grapheme_width(out.icons.rule).max(1);
    let count = (out.width as usize) / rule_cells;
    let piece = DecorPiece {
        first: out.icons.rule.repeat(count),
        cont: String::new(),
        scope,
    };
    // Appended through the same chokepoint as every other producer, never
    // assigned wholesale: a rule inside a blockquote shares its line with
    // the quote-bar piece already pushed, and clobbering the slot would
    // silently drop that bar. The wrap layer's rule-clamp truncates the
    // combined pieces to the available width, so bar + full-width rule
    // degrades by shaving trailing rule cells, not by losing the bar.
    push_piece(out, line, piece);
    if let Some(Some(decor)) = out.decors.get_mut(line) {
        decor.is_rule = true;
    }
}

/// A thematic break's full-width rule (plan WP2.S5/B1). See
/// `push_rule_decor`'s docs for the shared shape.
pub(crate) fn push_hr_decor(out: &mut EmitOut, line: usize) {
    push_rule_decor(out, line, super::style::hr_scope());
}

/// A setext heading's underline row, painted as a full-width rule in the
/// heading's own style rather than the thematic-break style — the user-
/// decided target behavior: a concealed setext heading hides its raw
/// `===`/`---` and shows a rule in `heading_style(level)`. See
/// `push_rule_decor`'s docs for the shared shape.
pub(crate) fn push_heading_rule_decor(out: &mut EmitOut, line: usize, level: u8) {
    push_rule_decor(out, line, heading_style(level));
}
