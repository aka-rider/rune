//! The `Cell` model + the buffer-content -> `Cell` walk (split out of
//! `render` per the 500-line budget): one visible terminal cell's shape, the width
//! chokepoint every span-to-cells walk shares, and the two public entry
//! points (`segment_cells`/`segment_geometry`) `render::build_rows` and
//! `commands::mouse`'s hit-testing call into.

use std::ops::Range;

use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;

use rune_syntax::ScopeId;
use rune_syntax::SyntaxSpan;
use rune_syntax::wrap::{control_aware_width, grapheme_width, rune_width_with_tab};

use crate::theme::Theme;

/// One visible terminal cell, with its buffer-byte provenance. `text` holds
/// the cell's WHOLE grapheme cluster (`unicode_segmentation::graphemes`),
/// not a single `char`: a ZWJ family emoji or a skin-tone-modified emoji is
/// several `char`s (joiner/modifier codepoints included) that together form
/// ONE user-perceived character occupying ONE cell — see `push_grapheme_
/// cells`'s docs for why splitting a cluster across multiple `Cell`s
/// corrupts the terminal output. `-1` marks a cell with no direct buffer
/// correspondence — decorative/synthetic (a synthetic EOL cursor cell
/// still carries its actual cursor byte offset, never `-1`, since it DOES
/// have a precise buffer position). A genuinely decorative cell — a
/// synthetic table border, or a line's own heading-icon/bullet/quote-bar/
/// hr-rule prefix — always uses `-1`, and every consumer that walks `Cell`s
/// keyed on buffer position (the highlight overlay, the selection/caret
/// overlays, mouse hit-testing) skips a negative `buf_offset` rather than
/// resolving it to a byte.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    pub width: u8,
    pub style: Style,
    pub buf_offset: i64,
}

/// Semantic `ScopeId` -> `ratatui::style::Style` — delegates to
/// `Theme::scope_style` (the theme is the ONE style source,
/// replacing an earlier `styles::markdown`'s role). Kept as a thin wrapper
/// (rather than calling `theme.scope_style` directly from `segment_cells`
/// below) so `render.rs` still owns the ONE call site `tests/tui_render.rs`
/// documents itself against.
pub fn style_for(theme: &Theme, id: ScopeId) -> Style {
    theme.scope_style(id)
}

/// Maps a C0 control code (`0x00..=0x1f`) or DEL (`0x7f`) to its Unicode
/// "control picture" glyph (`U+2400..=U+2421`) so a raw control byte never
/// reaches `ratatui::buffer::Cell::set_symbol`. ratatui-core's own
/// `cell_width()` asserts that a single-byte symbol is never
/// `u8::is_ascii_control` — feeding it a literal `\r`, `\x07`, `\x0c`, etc.
/// panics a debug build the instant that cell is diffed (an unsaved buffer
/// lost, with no recovery store in Phase 1) and silently corrupts the row
/// in a release build. `\n`/`\r` and `\t` are handled separately in
/// `push_grapheme_cells` below and never reach this function; any other
/// Unicode control category (e.g. C1, `0x80..=0x9f`) has no assigned
/// control-picture glyph and falls back to the replacement character.
fn control_placeholder(ch: char) -> char {
    match ch as u32 {
        0x00..=0x1f => char::from_u32(0x2400 + ch as u32).unwrap_or('\u{FFFD}'),
        0x7f => '\u{2421}',
        _ => '\u{FFFD}',
    }
}

/// The ONE width chokepoint `segment_cells` uses to advance its running
/// visual column — built on the exact same functions (`rune_width_with_tab`,
/// `control_aware_width`) `rune_syntax::wrap` uses for its own greedy line
/// breaking and for `WrapSnapshot::visual_col`/`byte_col_from_visual`. If
/// this ever drifted from wrap's width math, a row's `Cell` columns would no
/// longer line up with the column `visual_col` computes for the same
/// content, and `place_caret` would land the caret on the wrong cell (e.g.
/// after "abc" instead of on "a" for `"\tabc"` if tabs were treated as
/// width 1 here but width-to-next-4-stop there). `pub(crate)` (not private)
/// because `render::image`'s `centered_cells` is a second caller: it walks
/// arbitrary user text (a document's file name, an inline embed's link
/// target) that is just as capable of containing a raw control byte as any
/// buffer content is, and needs the same `control_placeholder` substitution
/// this function already performs rather than re-implementing it.
///
/// Operates on ONE GRAPHEME CLUSTER at a time (`unicode_segmentation::
/// graphemes(text, true)`, called by `segment_cells` below), not one `char`
/// — a ZWJ family emoji (`👨‍👩‍👧‍👦`, 7 codepoints joined by U+200D) or a
/// skin-tone-modified emoji (`👋🏽`, base + Fitzpatrick modifier) is many
/// `char`s but ONE user-perceived character. Splitting it across multiple
/// `Cell`s used to corrupt the display: `ratatui::buffer::Buffer`'s own
/// diffing (`BufferDiff`, ratatui-core) treats any cell whose `cell_width()`
/// (derived from whatever's actually stored in that buffer position) is `>
/// 1` as covering the NEXT `cell_width() - 1` columns too, and silently
/// SKIPS diffing/redrawing them — "we're assuming buffers are well-formed,
/// that is no double-width cell is followed by a non-blank cell" (ratatui's
/// own `Buffer::diff_iter` docs). Emitting each ZWJ-sequence codepoint as
/// its own `Cell` put a REAL (not blank) codepoint in exactly the column
/// ratatui's diffing treats as a wide cell's hidden continuation — that
/// codepoint's bytes never reach the real terminal, and every subsequent
/// column on the row shifts, which is exactly the "stray joiner/emoji
/// fragments" and "extra spaces inserted mid-sequence" corruption a
/// screen-capture comparison caught. `blit` (below) closes the other half: it
/// explicitly resets the buffer cells a wide `Cell` covers, matching
/// `Buffer::set_stringn`'s own reset loop, so ratatui's well-formedness
/// assumption actually holds for cells THIS code writes directly via
/// `cell_mut` (which, unlike `set_string`, does no such reset on its own).
///
/// `visual_col` bookkeeping for a MULTI-rune cluster comes from `rune_syntax::
/// wrap::grapheme_width` — the exact same function `wrap_line`'s own greedy
/// line-breaking and `WrapSnapshot::visual_col`/`byte_col_from_visual` call
/// for the same cluster — so the shared-chokepoint property (wrap's width
/// math and render's width math must stay identical) is one function call,
/// not two independently-written sums that merely happen to agree today.
///
/// `\n`/`\r` are dropped entirely — zero cells, zero width, matching
/// `control_aware_width`'s own `0` for them. `\t` expands into `width`
/// single-width space cells, ALL carrying the tab's own `buf_offset`, so
/// the caret can land on any of the tab's columns and still map back to the
/// tab byte; `width` is computed against `*visual_col`, so it lands on the
/// same 4-stop boundary `rune_width_with_tab` would compute for wrap
/// breaking or cursor coordinate conversion. A single non-tab/newline
/// control char is replaced with a safe placeholder glyph
/// (`control_placeholder`) at its `control_aware_width` (always `1` — no
/// width change, only a render-safety substitution). Neither case can ever
/// apply to a genuine multi-codepoint cluster (grapheme segmentation never
/// joins a control char to a neighboring one), so both are single-char
/// fast paths ahead of the generic (single- or multi-codepoint) case.
pub(crate) fn push_grapheme_cells(
    cells: &mut Vec<Cell>,
    visual_col: &mut usize,
    grapheme: &str,
    buf_offset: i64,
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
                        text: " ".to_string(),
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
                    text: control_placeholder(first).to_string(),
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
        text: grapheme.to_string(),
        width: width as u8,
        style,
        buf_offset,
    });
    *visual_col += width;
}

/// One wrap segment's spans -> its `Cell` row. A `Substituted` span maps
/// each GRAPHEME CLUSTER through its `cell_map` (its text is NOT
/// byte-for-byte its buffer range — delimiters were dropped), using the
/// cluster's FIRST char's offset (`push_grapheme_cells`'s docs: a cluster is
/// one visual unit, so it carries one `buf_offset`); an `Identical` span
/// walks `text` directly from its `range`'s start since its text IS
/// byte-for-byte its buffer range. `visual_col` accumulates across the WHOLE
/// segment (a wrap row), reset to `0` at the segment's start — the same
/// per-row-relative convention `wrap.rs`'s `wrap_line`/`visual_col` use, so
/// a tab's width agrees with both regardless of which span it's in.
pub fn segment_cells(theme: &Theme, content: &str, spans: &[SyntaxSpan]) -> Vec<Cell> {
    segment_cells_with(content, spans, |scope| style_for(theme, scope))
}

/// A segment's cells with styling elided — for callers that read only the
/// GEOMETRY (`Cell::width`, `Cell::buf_offset`), neither of which depends
/// on the theme. `commands::mouse`'s hit-testing is the one such caller:
/// it already holds the document mutably, so borrowing `App::theme` purely
/// to reach widths it would then discard is both a borrow conflict and a
/// lie about what a click depends on.
pub fn segment_geometry(content: &str, spans: &[SyntaxSpan]) -> Vec<Cell> {
    segment_cells_with(content, spans, |_| Style::default())
}

/// The ONE cell walk both entry points above share — `style_of` is its
/// only theme-dependent input, so the styled and geometry-only paths can
/// never drift in how they measure a row. Takes a plain `&[SyntaxSpan]`,
/// not a whole `WrapSegment`: a `DisplayRow`'s synthesised border
/// spans have no backing `WrapSegment` to read `.spans` off of.
fn segment_cells_with(
    content: &str,
    spans: &[SyntaxSpan],
    style_of: impl Fn(ScopeId) -> Style,
) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut visual_col = 0usize;
    for sp in spans {
        let style = style_of(sp.scope());
        match sp {
            SyntaxSpan::Substituted { text, cell_map, .. } => {
                // A producer bug (cell_map built from different text than
                // what's emitted) would make a grapheme's offset lookup
                // fall past the end of `cell_map` — an ordinary shipped
                // build degrades gracefully (the `-1` "no buffer
                // correspondence" sentinel, same as any other decorative
                // cell).
                let mut char_idx = 0usize;
                for grapheme in text.graphemes(true) {
                    let offset = cell_map.get(char_idx).copied().unwrap_or(-1);
                    push_grapheme_cells(&mut cells, &mut visual_col, grapheme, offset, style);
                    char_idx += grapheme.chars().count();
                }
            }
            SyntaxSpan::Identical { .. } => {
                let mut offset = sp.range().start;
                for grapheme in sp.text(content).graphemes(true) {
                    push_grapheme_cells(
                        &mut cells,
                        &mut visual_col,
                        grapheme,
                        offset as i64,
                        style,
                    );
                    offset += grapheme.len();
                }
            }
        }
    }
    cells
}

/// Patches `style` onto every cell in `rows` whose `buf_offset` falls
/// inside `range` — the one byte-range background painter every
/// region-highlight pass in this crate shares (merge mode's per-block
/// backgrounds, the search bar's per-match highlight): a decorative cell
/// (`buf_offset < 0`) is never a candidate, since it claims no buffer byte
/// to compare `range` against.
pub(crate) fn paint_range(rows: &mut [Vec<Cell>], range: Range<usize>, style: Style) {
    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            if cell.buf_offset < 0 {
                continue;
            }
            let offset = cell.buf_offset as usize;
            if range.contains(&offset) {
                cell.style = cell.style.patch(style);
            }
        }
    }
}
