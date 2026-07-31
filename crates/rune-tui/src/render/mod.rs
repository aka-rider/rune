//! The Cell model + blit (plan Context, "Cell model"): `DisplaySnapshot`
//! wrap rows -> `Vec<Vec<Cell>>` (Rendered spans via `cell_map`, Revealed
//! spans via byte arithmetic — port of Go's cell builder) -> overlays keyed on
//! `buf_offset` (cursor reverse-video, selection background, synthetic EOL
//! cursor cell, Go parity) -> blit into `frame.buffer_mut()`.
//! The terminal cursor stays hidden (`term::Guard::new`); the caret drawn
//! here IS the cursor, Go parity.

pub(crate) mod decor;
mod overlay;
pub mod title;

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType};
use unicode_segmentation::UnicodeSegmentation;

use rune_md::element::doc::ViewSnapshots;
use rune_syntax::ScopeId;
use rune_syntax::SyntaxSpan;
use rune_syntax::wrap::{control_aware_width, grapheme_width, rune_width_with_tab};

use crate::app::App;
use crate::banner;
use crate::pane::Pane;
use crate::theme::Theme;

/// One visible terminal cell, with its buffer-byte provenance. `text` holds
/// the cell's WHOLE grapheme cluster (`unicode_segmentation::graphemes`),
/// not a single `char`: a ZWJ family emoji or a skin-tone-modified emoji is
/// several `char`s (joiner/modifier codepoints included) that together form
/// ONE user-perceived character occupying ONE cell — see `push_grapheme_
/// cells`'s docs for why splitting a cluster across multiple `Cell`s
/// corrupts the terminal output. `-1` marks a cell with no direct buffer
/// correspondence — decorative/synthetic (port of Go's `CellMapping`
/// sentinel, reused here for the same reason: a synthetic EOL cursor cell
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
/// `Theme::scope_style` (plan WP4: the theme is the ONE style source,
/// replacing `styles::markdown`'s pre-WP4 role). Kept as a thin wrapper
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
/// width 1 here but width-to-next-4-stop there).
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
/// fragments" and "extra spaces inserted mid-sequence" corruption the
/// parity harness caught. `blit` (below) closes the other half: it
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
/// `\n`/`\r` are dropped entirely — zero cells, zero width — matching Go's
/// `cell.go` (`if r == '\n' || r == '\r' { continue }`) and
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
fn push_grapheme_cells(
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
/// never drift in how they measure a row. Takes a plain `&[SyntaxSpan]`
/// (WP3), not a whole `WrapSegment`: a `DisplayRow`'s synthesised border
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
                // cell), per CONSTITUTION §1.3.
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

/// Builds the visible `Vec<Vec<Cell>>` for the editor viewport: the DISPLAY
/// rows (WP3: wrap rows plus synthesised table borders) in `[scroll_row,
/// scroll_row + height)`, with cursor/selection overlays applied.
/// `viewport.scroll_row` is a DISPLAY row index — `Document::scroll_to_cursor`
/// is its single writer and converts through `DisplaySnapshot::wrap_to_display`
/// before ever assigning it (see that function's docs).
pub fn build_rows(view: &ViewSnapshots, app: &App) -> Vec<Vec<Cell>> {
    let doc = app.active_doc();
    let viewport = &doc.viewport;
    let content = doc.buffer.content();
    let mut rows: Vec<Vec<Cell>> = crate::viewport::visible_rows(view.display.rows(), viewport)
        .map(|row| {
            // WP4.S2: the row's own decoration (heading icon / bullet /
            // quote bar / hr rule) is prepended BEFORE the overlay walks
            // below run — those walks all skip `buf_offset < 0`
            // (the overlay module documents that skip), so a decor prefix never competes
            // for highlight/selection/caret painting the way a real cell
            // would.
            let mut cells = decor::decor_row_cells(&app.theme, row);
            cells.extend(segment_cells(&app.theme, content, &row.spans));
            cells
        })
        .collect();

    // Plan WP5.S5: the tree-sitter overlay paints token colours BEFORE the
    // cursor overlays below, so a selection background or the caret's
    // reverse-video always wins over a token's foreground. D6 (syntax-
    // highlighting-latency plan): a code document with a retained tree is
    // queried fresh every frame, scoped to exactly the bytes just rendered
    // — `visible_byte_range` derives the same window `apply_highlight_
    // spans` itself would scan, so the query never does wasted work outside
    // it. `highlight_range` returning `None` (the language no longer
    // resolves against the registry — should not happen for a tree `parse`
    // itself produced, but not assumed) degrades to the stored fallback
    // spans exactly like a document with no tree at all.
    let queried;
    let spans: &[(Range<usize>, ScopeId)] = match &doc.highlight.tree {
        Some(tree) => match overlay::visible_byte_range(&rows)
            .and_then(|range| rune_ts::highlight_range(tree, range))
        {
            Some(result) => {
                queried = result.spans;
                &queried
            }
            None => &doc.highlight.spans,
        },
        None => &doc.highlight.spans,
    };
    overlay::apply_highlight_spans(&mut rows, spans, &app.theme);

    overlay::apply_cursor_overlays(
        doc.shows_caret(),
        &mut rows,
        view,
        &doc.cursors,
        &doc.buffer,
        viewport.scroll_row,
        &app.theme,
    );
    rows
}

// `apply_cursor_overlays`, `highlight_selection`, `place_caret` and
// `apply_highlight_spans` moved to `overlay.rs` (§1.6 budget) — `build_rows`
// above calls them through `overlay::`.

/// Blits `app.view`'s current snapshot into the editor rect, and every
/// other chrome rect `layout::geometry` computes (plan WP3.S8: `draw`
/// itself no longer computes any split — it consumes `Geometry`, the one
/// chokepoint every rect comes from, so it can never disagree with
/// `App::relayout`'s or `explorer`/`opentabs`'s own idea of the same
/// rects).
///
/// Render order is load-bearing (plan gotcha 16, WP4.S2/S5): the center
/// `Block` must paint before the editor rows are blitted (its border
/// spans the WHOLE `geo.center` rect, including where the editor sits one
/// cell in), and the breadcrumb overlay must run after both — it splices
/// directly onto the border row the `Block` already painted, exactly the
/// ordering Go documents at `workspace_view.go`.
pub fn draw(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let geo = crate::layout::geometry(area, app);

    // `draw_left_pane` itself no-ops on `geo.left_block == None` (plan
    // WP13.S6: the review-caught redundant guard used to re-check the same
    // condition here first).
    draw_left_pane(app, &geo, frame);

    if geo.center_bordered {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(if app.focus() == Pane::Editor {
                app.theme.chrome.active_border
            } else {
                app.theme.chrome.inactive_border
            });
        frame.render_widget(block, geo.center);
    }

    if let Some(title_area) = geo.title {
        title::draw(app, title_area, frame);
    }

    if let Some(view) = &app.active_doc().view {
        let rows = build_rows(view, app);
        blit(&rows, geo.editor, frame);
    }

    if geo.center_bordered {
        crate::breadcrumb::overlay(app, geo.center, app.focus() == Pane::Editor, frame);
    }

    // (b) The one banner delegation (plan WP3.S3) — all of its own cell
    // building lives in `banner.rs`, never here.
    if let Some(banner_area) = geo.banner {
        banner::draw(app, banner_area, frame);
    }
    crate::footer::draw(app, geo.footer, frame);
}

/// The left column: ONE titled, bordered block (" Files ") holding both
/// panes, with the Open Tabs section introduced by an in-block divider row
/// rather than a second border. This owns only that border and its
/// focus-colored style; every rect it paints into comes from `Geometry`
/// rather than a `Layout::split` of its own.
///
/// The single border's color tracks the EXPLORER's focus, since the block
/// is titled for it; the Tabs pane signals its own focus through the
/// divider row's color and the cursor prefix on its rows instead.
///
/// Render order is load-bearing: the block paints first — its border spans
/// the whole rect, including the columns the inner content sits inside —
/// then the Explorer rows, the divider, and the tab rows go on top.
fn draw_left_pane(app: &App, geo: &crate::layout::Geometry, frame: &mut Frame) {
    let Some(left_area) = geo.left_block else {
        return;
    };

    // The Explorer can now be collapsed independently of the column
    // itself (its own vertical splitter dragged to the top): when it has
    // no rows to draw into, titling the block " Files " would claim a
    // pane that isn't there, so the block's title follows what's actually
    // showing instead of assuming the Explorer always is.
    let title = if geo.explorer_inner.height == 0 {
        " Open "
    } else {
        " Files "
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(if app.focus() == Pane::Explorer {
            app.theme.chrome.active_border
        } else {
            app.theme.chrome.inactive_border
        })
        .title(title);
    frame.render_widget(block, left_area);

    if geo.explorer_inner.height > 0 {
        crate::explorer::draw(app, geo.explorer_inner, frame);
    }
    if let Some(divider) = geo.tabs_divider {
        crate::opentabs::draw_divider(app, divider, frame);
    }
    crate::opentabs::draw(app, geo.tabs_inner, frame);
}

/// Writes `rows` into `frame.buffer_mut()` starting at `area`'s top-left
/// corner, clipping to `area`'s bounds.
///
/// A `Cell` wider than 1 column (a wide CJK char, or a multi-codepoint
/// grapheme cluster like a ZWJ emoji sequence — `push_grapheme_cells`'s
/// docs) needs its OWN width's worth of buffer columns explicitly reset
/// (`ratatui::buffer::Cell::reset`), not just skipped over: `cell_mut`
/// writes one column at a time and never touches its neighbors, unlike
/// `Buffer::set_stringn` (which this code deliberately doesn't use — it
/// only ever writes ONE known-width symbol at a time, not a whole string to
/// re-measure), so without this loop the "continuation" column(s) a wide
/// cell covers keep whatever a PRIOR frame happened to leave there.
/// Ratatui's own diffing (`BufferDiff`, ratatui-core) silently skips
/// re-examining exactly that many columns after any cell whose OWN
/// `cell_width()` is `> 1` — "we're assuming buffers are well-formed, that
/// is no double-width cell is followed by a non-blank cell" — so leftover,
/// non-blank content there would never even reach the real terminal's
/// diff/redraw, breaking the ZWJ fix at the last step. Resetting by THIS
/// `Cell`'s own `width` (not by re-deriving `cell_width()` from whatever
/// symbol ends up stored) is always at least as wide as ratatui's own
/// derivation ever needs, because `control_aware_width` only ever clamps a
/// zero-or-negative-width codepoint UP to 1, never down — so this cell's
/// own width sum can never fall short of what ratatui independently
/// computes for the same text, and the render/wrap width-math identity
/// (this module's other load-bearing property) stays intact without
/// needing to force ratatui's own width derivation.
pub fn blit(rows: &[Vec<Cell>], area: Rect, frame: &mut Frame) {
    let buf = frame.buffer_mut();
    let right = area.x.saturating_add(area.width);
    for (row_idx, row) in rows.iter().enumerate() {
        let y = area.y.saturating_add(row_idx as u16);
        if y >= area.y.saturating_add(area.height) {
            break;
        }
        let mut x = area.x;
        for cell in row {
            if x >= right {
                break;
            }
            let width = u16::from(cell.width.max(1));
            // WP13.S2: a cell that *starts* inside `area` can still not
            // *fit* — a double-width glyph landing on the last column
            // would need a continuation cell past `right` that this loop
            // never writes, leaving the border's own cell un-reset there
            // (ratatui's diffing then never revisits it, so the gap
            // persists across frames — the resize-race defect this guards
            // against). Substitute a single blank cell instead of the
            // glyph whenever it wouldn't fully fit.
            let fits = x.saturating_add(width) <= right;
            if let Some(target) = buf.cell_mut((x, y)) {
                if fits {
                    target.set_symbol(&cell.text);
                } else {
                    target.set_symbol(" ");
                }
                target.set_style(cell.style);
            }
            if fits {
                for dx in 1..width {
                    let cx = x.saturating_add(dx);
                    if cx >= right {
                        break;
                    }
                    if let Some(cont) = buf.cell_mut((cx, y)) {
                        cont.reset();
                    }
                }
            }
            x = x.saturating_add(width);
        }
    }
}
