use rune_core::coords::{DisplayRow, WrapPoint, WrapRow};
use rune_md::element::doc::ViewSnapshots;

use crate::app::App;
use crate::document::Document;
use crate::render;

/// `(buffer offset, desired_col)` for a click at `(row, col)` relative to
/// the editor rect — `None` before the document's first sync, when `doc.view`
/// is still unset.
pub(crate) fn hit_test(app: &App, doc: &Document, row: u16, col: u16) -> Option<(usize, usize)> {
    let view = doc.view.as_ref()?;
    let offset = offset_at(app, doc, view, row, col)?;
    Some((offset, desired_col_at(doc, view, offset)))
}

fn offset_at(app: &App, doc: &Document, view: &ViewSnapshots, row: u16, col: u16) -> Option<usize> {
    let total = view.display.total_rows();
    if total == 0 {
        return Some(0);
    }
    let display_row = (doc.viewport.scroll_row + row as usize).min(DisplayRow(total - 1));
    let display_ref = view.display.rows().get(display_row.0);
    if display_ref.is_some_and(|r| r.synthetic) {
        return None;
    }
    let wrap_row = view.display.display_to_wrap(display_row);
    let content = doc.buffer.content();

    // An embed row showing placeholder pixels has no per-cell `buf_offset`
    // to walk toward, so a click anywhere on it resolves to the row's own
    // wrap-segment start.
    if let Some(image_ref) = display_ref.and_then(|r| r.image.clone())
        && image_ref.target.is_some()
        && let Some(cells) =
            crate::render::image::row_cells(app, doc, &image_ref, doc.viewport.width)
    {
        return if (col as usize) < cells.len() {
            Some(row_start_offset(doc, view, wrap_row))
        } else {
            None
        };
    }

    Some(offset_at_ordinary(
        doc,
        view,
        display_row,
        wrap_row,
        content,
        col,
    ))
}

fn row_start_offset(doc: &Document, view: &ViewSnapshots, wrap_row: WrapRow) -> usize {
    let content = doc.buffer.content();
    let syntax_point = view.wrap.wrap_to_syntax(
        content,
        WrapPoint {
            row: wrap_row.0,
            col: 0,
        },
    );
    let buffer_point = view.syntax.syntax_to_buffer(syntax_point);
    doc.buffer.line_col_to_offset(buffer_point)
}

fn offset_at_ordinary(
    doc: &Document,
    view: &ViewSnapshots,
    display_row: DisplayRow,
    wrap_row: WrapRow,
    content: &str,
    col: u16,
) -> usize {
    let decor_width = view
        .display
        .rows()
        .get(display_row.0)
        .map_or(0, crate::render::decor::decor_cell_width) as usize;
    let col = (col as usize).saturating_sub(decor_width) as u16;

    let mut acc = 0usize;
    let mut first_content_offset: Option<u32> = None;
    if let Some(seg) = view.wrap.segments().get(wrap_row.0) {
        for cell in render::segment_geometry(content, &seg.spans) {
            first_content_offset = first_content_offset.or(cell.buf_offset);
            let width = cell.width as usize;
            // A width-0 cell (a lone zero-width rune) occupies no screen
            // column of its own — `acc + width == acc` — so the comparison
            // below falls through to whichever cell actually owns that
            // column, matching what a real terminal shows there.
            if (col as usize) < acc + width {
                return if let Some(offset) = cell.buf_offset {
                    offset as usize
                } else {
                    first_content_offset.unwrap_or(0) as usize
                };
            }
            acc += width;
        }
    }

    let seg_len = view.wrap.segment_len_at(wrap_row.0);
    let syntax_point = view.wrap.wrap_to_syntax(
        content,
        WrapPoint {
            row: wrap_row.0,
            col: seg_len,
        },
    );
    let buffer_point = view.syntax.syntax_to_buffer(syntax_point);
    doc.buffer.line_col_to_offset(buffer_point)
}

fn desired_col_at(doc: &Document, view: &ViewSnapshots, offset: usize) -> usize {
    let bp = doc.buffer.offset_to_line_col(offset);
    let sp = view.syntax.buffer_to_syntax(bp);
    let wp = view.wrap.syntax_to_wrap(sp);
    view.wrap.visual_col(doc.buffer.content(), wp.row, wp.col)
}
