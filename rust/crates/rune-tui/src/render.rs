//! The Cell model + blit (plan Context, "Cell model"): `DisplaySnapshot`
//! wrap rows -> `Vec<Vec<Cell>>` (Rendered spans via `cell_map`, Revealed
//! spans via byte arithmetic — port of
//! `pkg/ui/components/textedit/cell.go:59-126`) -> overlays keyed on
//! `buf_offset` (cursor reverse-video, selection background, synthetic EOL
//! cursor cell per Go `render.go:151-176`) -> blit into `frame.buffer_mut()`.
//! The terminal cursor stays hidden (`term::Guard::new`); the caret drawn
//! here IS the cursor, Go parity.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier as RtModifier, Style};
use ratatui::widgets::{Block, BorderType};

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::ViewSnapshots;
use rune_syntax::ScopeId;
use rune_syntax::SyntaxSpan;
use rune_syntax::wrap::{WrapSegment, control_aware_width, rune_width_with_tab};

use crate::app::App;
use crate::banner;
use crate::pane::Pane;
use crate::theme::Theme;

/// One visible terminal cell, with its buffer-byte provenance. `-1` marks a
/// cell with no direct buffer correspondence — decorative/synthetic (port of
/// `pkg/editor/display/cellmap.go`'s `CellMapping` sentinel, reused here for
/// the same reason: a synthetic EOL cursor cell still carries its actual
/// cursor byte offset, never `-1`, since it DOES have a precise buffer
/// position — only a genuinely decorative cell would use `-1`, and Phase 1
/// never produces one).
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub ch: char,
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
/// reaches `ratatui::buffer::Cell::set_char`. ratatui-core's `cell_width()`
/// `debug_assert!`s that a single-byte symbol is never
/// `u8::is_ascii_control` (`cell_width.rs:36`) — feeding it a literal `\r`,
/// `\x07`, `\x0c`, etc. panics a debug build the instant that cell is diffed
/// (an unsaved buffer lost, with no recovery store in Phase 1) and silently
/// corrupts the row in a release build. `\n`/`\r` and `\t` are handled
/// separately in `push_char_cells` below and never reach this function; any
/// other Unicode control category (e.g. C1, `0x80..=0x9f`) has no assigned
/// control-picture glyph and falls back to the replacement character.
fn control_placeholder(ch: char) -> char {
    match ch as u32 {
        0x00..=0x1f => char::from_u32(0x2400 + ch as u32).unwrap_or('\u{FFFD}'),
        0x7f => '\u{2421}',
        _ => '\u{FFFD}',
    }
}

/// The ONE width chokepoint `segment_cells` uses to advance its running
/// visual column — the exact same functions (`rune_width_with_tab`,
/// `control_aware_width`) `rune_syntax::wrap` uses for its own greedy line
/// breaking and for `WrapSnapshot::visual_col`/`byte_col_from_visual`. If
/// this ever drifted from wrap's width math, a row's `Cell` columns would no
/// longer line up with the column `visual_col` computes for the same
/// content, and `place_caret` would land the caret on the wrong cell (e.g.
/// after "abc" instead of on "a" for `"\tabc"` if tabs were treated as
/// width 1 here but width-to-next-4-stop there).
///
/// `\n`/`\r` are dropped entirely — zero cells, zero width — matching Go's
/// `cell.go:79,106` (`if r == '\n' || r == '\r' { continue }`) and
/// `control_aware_width`'s own `0` for them. `\t` expands into `width`
/// single-width space cells, ALL carrying the tab's own `buf_offset`, so
/// the caret can land on any of the tab's columns and still map back to the
/// tab byte; `width` is computed against `*visual_col`, so it lands on the
/// same 4-stop boundary `rune_width_with_tab` would compute for wrap
/// breaking or cursor coordinate conversion. Every other control char is
/// replaced with a safe placeholder glyph (`control_placeholder`) at its
/// `control_aware_width` (always `1` for a control char — no width change,
/// only a render-safety substitution).
fn push_char_cells(
    cells: &mut Vec<Cell>,
    visual_col: &mut usize,
    ch: char,
    buf_offset: i64,
    style: Style,
) {
    match ch {
        '\n' | '\r' => {}
        '\t' => {
            let width = rune_width_with_tab(ch, *visual_col);
            for _ in 0..width {
                cells.push(Cell {
                    ch: ' ',
                    width: 1,
                    style,
                    buf_offset,
                });
            }
            *visual_col += width;
        }
        _ if ch.is_control() => {
            let width = control_aware_width(ch);
            cells.push(Cell {
                ch: control_placeholder(ch),
                width: width as u8,
                style,
                buf_offset,
            });
            *visual_col += width;
        }
        _ => {
            let width = control_aware_width(ch);
            cells.push(Cell {
                ch,
                width: width as u8,
                style,
                buf_offset,
            });
            *visual_col += width;
        }
    }
}

/// One wrap segment's spans -> its `Cell` row. A `Substituted` span maps
/// each char through its `cell_map` (its text is NOT byte-for-byte its
/// buffer range — delimiters were dropped); an `Identical` span walks
/// `text` directly from its `range`'s start since its text IS byte-for-byte
/// its buffer range. `visual_col` accumulates across the WHOLE segment (a
/// wrap row), reset to `0` at the segment's start — the same
/// per-row-relative convention `wrap.rs`'s `wrap_line`/`visual_col` use, so
/// a tab's width agrees with both regardless of which span it's in.
pub fn segment_cells(theme: &Theme, content: &str, seg: &WrapSegment) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut visual_col = 0usize;
    for sp in &seg.spans {
        let style = style_for(theme, sp.scope());
        match sp {
            SyntaxSpan::Substituted { text, cell_map, .. } => {
                // A producer bug (cell_map built from different text than
                // what's emitted) would make `zip` below silently drop
                // whichever side is longer — an ordinary shipped build
                // degrades gracefully (renders only the min of the two,
                // same as `zip`'s normal behavior), per CONSTITUTION §1.3.
                // The `Substituted` variant itself now makes "text without
                // a matching cell_map" unrepresentable (plan WP2): both are
                // built together by `rune-md`'s one constructor, so the
                // mismatch this used to `debug_assert_eq!` against is no
                // longer a shape the type permits upstream — the `zip`
                // degradation below is the only defense left, and it's
                // unreachable in practice.
                for (ch, &offset) in text.chars().zip(cell_map.iter()) {
                    push_char_cells(&mut cells, &mut visual_col, ch, offset, style);
                }
            }
            SyntaxSpan::Identical { .. } => {
                let mut offset = sp.range().start;
                for ch in sp.text(content).chars() {
                    push_char_cells(&mut cells, &mut visual_col, ch, offset as i64, style);
                    offset += ch.len_utf8();
                }
            }
        }
    }
    cells
}

/// Builds the visible `Vec<Vec<Cell>>` for the editor viewport: the wrap
/// rows in `[scroll_row, scroll_row + height)`, with cursor/selection
/// overlays applied.
pub fn build_rows(view: &ViewSnapshots, app: &App) -> Vec<Vec<Cell>> {
    let doc = app.active_doc();
    let viewport = &doc.viewport;
    let content = doc.buffer.content();
    let height = viewport.height as usize;
    let mut rows: Vec<Vec<Cell>> = view
        .wrap
        .segments()
        .iter()
        .skip(viewport.scroll_row)
        .take(height)
        .map(|seg| segment_cells(&app.theme, content, seg))
        .collect();

    apply_cursor_overlays(
        &mut rows,
        view,
        &doc.cursors,
        &doc.buffer,
        viewport.scroll_row,
        &app.theme,
    );
    rows
}

fn apply_cursor_overlays(
    rows: &mut [Vec<Cell>],
    view: &ViewSnapshots,
    cursors: &CursorSet,
    buf: &Buffer,
    scroll_row: usize,
    theme: &Theme,
) {
    for cursor in cursors.all() {
        if cursor.has_selection() {
            let (start, end) = cursor.selection_range();
            highlight_selection(rows, start, end, theme);
        }

        let buffer_point = buf.offset_to_line_col(cursor.position);
        let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
        if wrap_point.row < scroll_row {
            continue;
        }
        let Some(row) = rows.get_mut(wrap_point.row - scroll_row) else {
            continue;
        };
        let visual_col = view
            .wrap
            .visual_col(buf.content(), wrap_point.row, wrap_point.col);
        place_caret(row, visual_col, cursor.position);
    }
}

fn highlight_selection(rows: &mut [Vec<Cell>], start: usize, end: usize, theme: &Theme) {
    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            if cell.buf_offset >= 0 {
                let offset = cell.buf_offset as usize;
                if offset >= start && offset < end {
                    // Go `Selection` (`styles.go:196`, WP2.S2 migration).
                    cell.style = cell.style.bg(theme.chrome.selection_bg);
                }
            }
        }
    }
}

/// Reverse-video the cell at `visual_col`, or — if the caret sits past the
/// last visible char on this row — append a synthetic EOL cursor cell (port
/// of Go `render.go:151-176`).
fn place_caret(row: &mut Vec<Cell>, visual_col: usize, buf_offset: usize) {
    let mut col = 0usize;
    for cell in row.iter_mut() {
        if col == visual_col {
            cell.style = cell.style.add_modifier(RtModifier::REVERSED);
            return;
        }
        col += cell.width.max(1) as usize;
    }
    row.push(Cell {
        ch: ' ',
        width: 1,
        style: Style::default().add_modifier(RtModifier::REVERSED),
        buf_offset: buf_offset as i64,
    });
}

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
/// ordering Go documents at `workspace_view.go:327-330`.
pub fn draw(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let geo = crate::layout::geometry(area, app);

    // `explorer_block.is_some()` iff `tabs_block.is_some()` (plan WP3.S1:
    // both come from the same `left_pane_width` branch) — `draw_left_pane`
    // itself re-checks both and no-ops if either is `None`.
    if geo.explorer_block.is_some() {
        draw_left_pane(app, &geo, frame);
    }

    if geo.center_bordered {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(if app.focus == Pane::Editor {
                app.theme.chrome.active_border
            } else {
                app.theme.chrome.inactive_border
            });
        frame.render_widget(block, geo.center);
    }

    if let Some(title_area) = geo.title {
        crate::title::draw(app, title_area, frame);
    }

    if let Some(view) = &app.active_doc().view {
        let rows = build_rows(view, app);
        blit(&rows, geo.editor, frame);
    }

    if geo.center_bordered {
        crate::breadcrumb::overlay(app, geo.center, app.focus == Pane::Editor, frame);
    }

    // (b) The one banner delegation (plan WP3.S3) — all of its own cell
    // building lives in `banner.rs`, never here.
    if let Some(banner_area) = geo.banner {
        banner::draw(app, banner_area, frame);
    }
    crate::footer::draw(app, geo.footer, frame);
}

/// The left column's two titled, bordered blocks (plan WP2.S5): Explorer on
/// top ("Files"), Open Tabs below ("Open" — its own content lands in WP5).
/// This owns only the border and the focus-colored border style; its two
/// block rects and their inner rects all come from `Geometry` (plan
/// WP3.S8) rather than computing its own `Layout::split`. The Explorer's
/// own row content (root path, entries) is delegated to `explorer::draw`
/// at the block's INNER rect (plan WP4.S6) — the one content-owning call
/// this function makes.
fn draw_left_pane(app: &App, geo: &crate::layout::Geometry, frame: &mut Frame) {
    let (Some(explorer_area), Some(tabs_area)) = (geo.explorer_block, geo.tabs_block) else {
        return;
    };

    let border_style = |pane: Pane| {
        if app.focus == pane {
            app.theme.chrome.active_border
        } else {
            app.theme.chrome.inactive_border
        }
    };

    let explorer_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border_style(Pane::Explorer))
        .title("Files");
    frame.render_widget(explorer_block, explorer_area);
    crate::explorer::draw(app, geo.explorer_inner, frame);

    let tabs_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border_style(Pane::Tabs))
        .title("Open");
    frame.render_widget(tabs_block, tabs_area);
    crate::opentabs::draw(app, geo.tabs_inner, frame);
}

/// Writes `rows` into `frame.buffer_mut()` starting at `area`'s top-left
/// corner, clipping to `area`'s bounds.
pub fn blit(rows: &[Vec<Cell>], area: Rect, frame: &mut Frame) {
    let buf = frame.buffer_mut();
    for (row_idx, row) in rows.iter().enumerate() {
        let y = area.y.saturating_add(row_idx as u16);
        if y >= area.y.saturating_add(area.height) {
            break;
        }
        let mut x = area.x;
        for cell in row {
            if x >= area.x.saturating_add(area.width) {
                break;
            }
            if let Some(target) = buf.cell_mut((x, y)) {
                target.set_char(cell.ch);
                target.set_style(cell.style);
            }
            x = x.saturating_add(cell.width.max(1) as u16);
        }
    }
}
