//! The Cell model + blit (plan Context, "Cell model"): `DisplaySnapshot`
//! wrap rows -> `Vec<Vec<Cell>>` (Rendered spans via `cell_map`, Revealed
//! spans via byte arithmetic — port of
//! `pkg/ui/components/textedit/cell.go:59-126`) -> overlays keyed on
//! `buf_offset` (cursor reverse-video, selection background, synthetic EOL
//! cursor cell per Go `render.go:151-176`) -> blit into `frame.buffer_mut()`.
//! The terminal cursor stays hidden (`term::Guard::new`); the caret drawn
//! here IS the cursor, Go parity.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier as RtModifier, Style};
use ratatui::widgets::{Block, BorderType};

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::RevealState;
use rune_md::element::doc::ViewSnapshots;
use rune_md::emit::StyleId;
use rune_md::wrap::{WrapSegment, control_aware_width, rune_width_with_tab};

use crate::app::App;
use crate::banner;
use crate::pane::Pane;
use crate::styles;

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

/// Semantic `StyleId` -> `ratatui::style::Style` — delegates to
/// `styles::markdown` (plan WP2.S2, critic R2: "exactly one style source").
/// Kept as a thin wrapper (rather than calling `styles::markdown` directly
/// from `segment_cells` below) so `render.rs` still owns the ONE call site
/// `tests/tui_render.rs` documents itself against.
pub fn style_for(id: StyleId) -> Style {
    styles::markdown(id)
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
/// `control_aware_width`) `rune_md::wrap` uses for its own greedy line
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

/// One wrap segment's spans -> its `Cell` row. Rendered spans map each char
/// through `cell_map` (their text is NOT byte-for-byte their buffer range —
/// delimiters were dropped); Revealed spans (and any span with no
/// `cell_map`) walk `text` directly from `buffer_start` since their text IS
/// byte-for-byte their buffer range. `visual_col` accumulates across the
/// WHOLE segment (a wrap row), reset to `0` at the segment's start — the
/// same per-row-relative convention `wrap.rs`'s `wrap_line`/`visual_col`
/// use, so a tab's width agrees with both regardless of which span it's in.
pub fn segment_cells(seg: &WrapSegment) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut visual_col = 0usize;
    for span in &seg.spans {
        let style = style_for(span.style);
        match (&span.state, &span.cell_map) {
            (RevealState::Rendered, Some(cell_map)) => {
                let char_count = span.text.chars().count();
                // A producer bug (cell_map built from different text than
                // what's emitted) would make `zip` below silently drop
                // whichever side is longer — surfaced here as a debug-mode
                // diagnostic rather than a silent row truncation; an
                // ordinary shipped build still degrades gracefully (renders
                // only the min of the two, same as `zip`'s normal
                // behavior), per CONSTITUTION §1.3.
                debug_assert_eq!(
                    char_count,
                    cell_map.len(),
                    "Rendered span's cell_map length ({}) does not match its text's char count ({char_count}) — a producer bug upstream in rune-md; truncating to the shorter of the two",
                    cell_map.len(),
                );
                for (ch, &offset) in span.text.chars().zip(cell_map.iter()) {
                    push_char_cells(&mut cells, &mut visual_col, ch, offset, style);
                }
            }
            _ => {
                let mut offset = span.buffer_start;
                for ch in span.text.chars() {
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
    let height = viewport.height as usize;
    let mut rows: Vec<Vec<Cell>> = view
        .wrap
        .segments()
        .iter()
        .skip(viewport.scroll_row)
        .take(height)
        .map(segment_cells)
        .collect();

    apply_cursor_overlays(
        &mut rows,
        view,
        &doc.cursors,
        &doc.buffer,
        viewport.scroll_row,
    );
    rows
}

fn apply_cursor_overlays(
    rows: &mut [Vec<Cell>],
    view: &ViewSnapshots,
    cursors: &CursorSet,
    buf: &Buffer,
    scroll_row: usize,
) {
    for cursor in cursors.all() {
        if cursor.has_selection() {
            let (start, end) = cursor.selection_range();
            highlight_selection(rows, start, end);
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
        let visual_col = view.wrap.visual_col(wrap_point.row, wrap_point.col);
        place_caret(row, visual_col, cursor.position);
    }
}

fn highlight_selection(rows: &mut [Vec<Cell>], start: usize, end: usize) {
    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            if cell.buf_offset >= 0 {
                let offset = cell.buf_offset as usize;
                if offset >= start && offset < end {
                    // Go `Selection` (`styles.go:196`, WP2.S2 migration).
                    cell.style = cell.style.bg(styles::SELECTION_BG);
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

/// Left-pane geometry (plan WP2.S5): the DEFAULT column width, the
/// smallest it may shrink to, and the smallest the editor pane may shrink
/// to in exchange. `left_pane_width` below is the pure query both `draw`'s
/// layout and any test asserting on the split use, so they can never
/// disagree on the number.
pub const DEFAULT_LEFT_PANE_W: u16 = 22;
pub const MIN_LEFT_PANE_W: u16 = 16;
pub const MIN_CENTER_W: u16 = 24;

/// The left-pane column width for a `total_width`-wide main area, or `None`
/// when the terminal is too narrow to give BOTH the left pane its minimum
/// AND the editor `MIN_CENTER_W` (plan WP2.S5: "if the terminal is too
/// narrow for both minimums, drop the left pane for that frame").
fn left_pane_width(total_width: u16) -> Option<u16> {
    if total_width < MIN_LEFT_PANE_W.saturating_add(MIN_CENTER_W) {
        return None;
    }
    let max_left = total_width.saturating_sub(MIN_CENTER_W);
    Some(DEFAULT_LEFT_PANE_W.min(max_left).max(MIN_LEFT_PANE_W))
}

/// Splits the frame into (optionally) a left pane column, the editor
/// viewport, and the footer row, and blits `app.view`'s current snapshot
/// into the editor area. The editor pane's own geometry is UNCHANGED from
/// pre-WP2 when `app.left_visible` is false (plan WP2.S5) — this `if`
/// exists entirely inside the space the left pane would otherwise borrow
/// from `main_area`, never touching `footer_area`.
pub fn draw(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    let main_area = chunks.first().copied().unwrap_or(area);
    let footer_area = chunks
        .get(1)
        .copied()
        .unwrap_or(Rect::new(area.x, area.y, area.width, 0));

    // (a) The banner Rect (plan WP3.S3): reserved directly above the
    // footer, full frame width, height capped at half the frame — shrinks
    // `main_area` (and so the editor's share of it) accordingly. `None`
    // when no modal is up, leaving `main_area`/the editor geometry below
    // exactly as pre-WP3.
    let (main_area, banner_area) = match app.modal.as_ref().map(banner::Modal::total_rows) {
        Some(total_rows) => {
            let banner_h = (total_rows as u16).min(area.height / 2);
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(banner_h)])
                .split(main_area);
            (
                rows.first().copied().unwrap_or(main_area),
                rows.get(1).copied(),
            )
        }
        None => (main_area, None),
    };

    let editor_area = if app.left_visible {
        match left_pane_width(main_area.width) {
            Some(left_w) => {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(left_w), Constraint::Min(0)])
                    .split(main_area);
                let left_area = cols.first().copied().unwrap_or(main_area);
                draw_left_pane(app, left_area, frame);
                cols.get(1).copied().unwrap_or(main_area)
            }
            None => main_area,
        }
    } else {
        main_area
    };

    let (title_area, breadcrumb_area, editor_area) = center_chrome_rows(editor_area);
    if let Some(area) = title_area {
        crate::title::draw(app, area, frame);
    }
    if let Some(area) = breadcrumb_area {
        crate::breadcrumb::draw(app, area, frame);
    }

    if let Some(view) = &app.active_doc().view {
        let rows = build_rows(view, app);
        blit(&rows, editor_area, frame);
    }
    // (b) The one banner delegation (plan WP3.S3) — all of its own cell
    // building lives in `banner.rs`, never here.
    if let Some(banner_area) = banner_area {
        banner::draw(app, banner_area, frame);
    }
    crate::footer::draw(app, footer_area, frame);
}

/// Reserves the center pane's title row and the breadcrumb row directly
/// under it (plan WP6.S2), shrinking the returned editor rect to match.
/// Both rows are reserved together, or neither: the plan's "≥ 4 rows tall"
/// gate already assumes a genuinely usable editor area survives underneath
/// (2 rows minimum) — a 3-row center pane has no room to spare for even a
/// title-only sliver, so it drops both rather than leaving one editor row
/// under a lone title. `render::draw` is left as pure rect math plus the two
/// delegation calls above — ALL text/styling lives in `title.rs`/
/// `breadcrumb.rs`.
fn center_chrome_rows(area: Rect) -> (Option<Rect>, Option<Rect>, Rect) {
    const CHROME_ROWS: u16 = 2;
    const MIN_CENTER_H: u16 = 4;
    if area.height < MIN_CENTER_H {
        return (None, None, area);
    }
    let title_area = Rect::new(area.x, area.y, area.width, 1);
    let breadcrumb_area = Rect::new(area.x, area.y + 1, area.width, 1);
    let editor_area = Rect::new(
        area.x,
        area.y + CHROME_ROWS,
        area.width,
        area.height - CHROME_ROWS,
    );
    (Some(title_area), Some(breadcrumb_area), editor_area)
}

/// The left column's two empty, titled, bordered blocks (plan WP2.S5):
/// Explorer on top ("Files" — a placeholder title until WP4 has a root dir
/// to name), Open Tabs below ("Open"). Contents land in WP4/WP5; this only
/// establishes the geometry and the focus-colored border.
fn draw_left_pane(app: &App, area: Rect, frame: &mut Frame) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let explorer_area = rows.first().copied().unwrap_or(area);
    let tabs_area = rows
        .get(1)
        .copied()
        .unwrap_or(Rect::new(area.x, area.y, area.width, 0));

    let border_style = |pane: Pane| {
        if app.focus == pane {
            styles::active_border()
        } else {
            styles::inactive_border()
        }
    };

    frame.render_widget(
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style(Pane::Explorer))
            .title("Files"),
        explorer_area,
    );
    frame.render_widget(
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style(Pane::Tabs))
            .title("Open"),
        tabs_area,
    );
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
