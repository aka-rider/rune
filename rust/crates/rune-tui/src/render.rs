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
use ratatui::style::{Color, Modifier as RtModifier, Style};

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::RevealState;
use rune_md::element::doc::ViewSnapshots;
use rune_md::emit::StyleId;
use rune_md::wrap::{WrapSegment, control_aware_width};

use crate::app::App;

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

/// Semantic `StyleId` -> `ratatui::style::Style` — the lipgloss-equivalent
/// theme (plan Context, "Parse": "the lipgloss-equivalent theme lives in
/// rune-tui"). Phase-1 placeholder palette; not itself under test — the
/// `StyleId` dispatch (which span gets which id) is what `tests/tui_render.rs`
/// asserts on.
pub fn style_for(id: StyleId) -> Style {
    let base = Style::default();
    match id {
        StyleId::Text => base,
        StyleId::H1 | StyleId::H2 | StyleId::H3 | StyleId::H4 | StyleId::H5 | StyleId::H6 => {
            base.fg(Color::Magenta).add_modifier(RtModifier::BOLD)
        }
        StyleId::Bold => base.add_modifier(RtModifier::BOLD),
        StyleId::Italic => base.add_modifier(RtModifier::ITALIC),
        StyleId::BoldItalic => base.add_modifier(RtModifier::BOLD | RtModifier::ITALIC),
        StyleId::Strike => base.add_modifier(RtModifier::CROSSED_OUT),
        StyleId::BoldStrike => base.add_modifier(RtModifier::BOLD | RtModifier::CROSSED_OUT),
        StyleId::ItalicStrike => base.add_modifier(RtModifier::ITALIC | RtModifier::CROSSED_OUT),
        StyleId::BoldItalicStrike => {
            base.add_modifier(RtModifier::BOLD | RtModifier::ITALIC | RtModifier::CROSSED_OUT)
        }
        StyleId::Code | StyleId::CodeFence => base.fg(Color::Yellow),
        StyleId::Link | StyleId::WikiLink => {
            base.fg(Color::Cyan).add_modifier(RtModifier::UNDERLINED)
        }
        StyleId::Blockquote => base.fg(Color::Gray),
        StyleId::ListMarker => base.fg(Color::Blue),
        StyleId::TaskMarker => base.fg(Color::Green),
        StyleId::Hr | StyleId::FrontmatterDim => base.fg(Color::DarkGray),
        StyleId::Verbatim => base.fg(Color::Gray),
    }
}

/// One wrap segment's spans -> its `Cell` row. Rendered spans map each char
/// through `cell_map` (their text is NOT byte-for-byte their buffer range —
/// delimiters were dropped); Revealed spans (and any span with no
/// `cell_map`) walk `text` directly from `buffer_start` since their text IS
/// byte-for-byte their buffer range.
pub fn segment_cells(seg: &WrapSegment) -> Vec<Cell> {
    let mut cells = Vec::new();
    for span in &seg.spans {
        let style = style_for(span.style);
        match (&span.state, &span.cell_map) {
            (RevealState::Rendered, Some(cell_map)) => {
                for (ch, &offset) in span.text.chars().zip(cell_map.iter()) {
                    cells.push(Cell {
                        ch,
                        width: control_aware_width(ch) as u8,
                        style,
                        buf_offset: offset,
                    });
                }
            }
            _ => {
                let mut offset = span.buffer_start;
                for ch in span.text.chars() {
                    cells.push(Cell {
                        ch,
                        width: control_aware_width(ch) as u8,
                        style,
                        buf_offset: offset as i64,
                    });
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
    let viewport = &app.editor.viewport;
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
        &app.editor.cursors,
        &app.editor.buffer,
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
                    cell.style = cell.style.bg(Color::DarkGray);
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

/// Splits the frame into the editor viewport (all but the last row) and the
/// status line, and blits `app.view`'s current snapshot into the buffer.
pub fn draw(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    let editor_area = chunks.first().copied().unwrap_or(area);
    let status_area = chunks
        .get(1)
        .copied()
        .unwrap_or(Rect::new(area.x, area.y, area.width, 0));

    if let Some(view) = &app.view {
        let rows = build_rows(view, app);
        blit(&rows, editor_area, frame);
    }
    crate::status::draw(app, status_area, frame);
}

/// Writes `rows` into `frame.buffer_mut()` starting at `area`'s top-left
/// corner, clipping to `area`'s bounds.
pub fn blit(rows: &[Vec<Cell>], area: Rect, frame: &mut Frame) {
    let buf = frame.buffer_mut();
    for (row_idx, row) in rows.iter().enumerate() {
        let y = area.y.saturating_add(row_idx as u16);
        if y >= area.y + area.height {
            break;
        }
        let mut x = area.x;
        for cell in row {
            if x >= area.x + area.width {
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
