//! Mouse hit-testing (split out of `mouse.rs`, §1.6 budget — WP9 pushed it
//! past 500 lines): `hit_test`/`offset_at` walk the clicked row's own
//! rendered cells (`render::segment_cells`) to find the buffer byte a
//! screen column corresponds to — the same cells `render::build_rows` just
//! blitted, so a click always resolves to exactly what's on screen.

use rune_core::coords::WrapPoint;
use rune_md::element::doc::ViewSnapshots;

use crate::app::App;
use crate::document::Document;
use crate::render;

/// `(buffer offset, desired_col)` for a click at `(row, col)` relative to
/// the editor rect — `None` before the document's first sync (`doc.view`
/// unset, never observed past the seeded initial `Msg::Resize` in
/// practice), or when the click landed on a synthesised table border row
/// (Go parity: `offset_at` returns `None` there too, so the gesture is a
/// complete no-op rather than moving the cursor to some nearby offset).
pub(super) fn hit_test(app: &App, doc: &Document, row: u16, col: u16) -> Option<(usize, usize)> {
    let view = doc.view.as_ref()?;
    let offset = offset_at(app, doc, view, row, col)?;
    Some((offset, desired_col_at(doc, view, offset)))
}

/// `row` is relative to the DISPLAY grid (WP3: what's actually on screen,
/// borders included) — clamped against `DisplaySnapshot::total_rows`, then
/// converted to the WRAP row the click's hit-tested content actually lives
/// at via `display_to_wrap`, since every coordinate below this point (cell
/// geometry, `wrap_to_syntax`) is wrap-space. A synthetic border row has no
/// such wrap row of its own to click into — returns `None` instead (see
/// `hit_test`'s docs).
fn offset_at(app: &App, doc: &Document, view: &ViewSnapshots, row: u16, col: u16) -> Option<usize> {
    let total = view.display.total_rows();
    if total == 0 {
        return Some(0);
    }
    let display_row = (doc.viewport.scroll_row + row as usize).min(total - 1);
    let display_ref = view.display.rows().get(display_row);
    if display_ref.is_some_and(|r| r.synthetic) {
        return None;
    }
    let wrap_row = view.display.display_to_wrap(display_row);
    let content = doc.buffer.content();

    // Plan WP9.S3: an EMBED anchor row (`r.image` is `Some` with a
    // `target` — the whole-document image producer's own rows are always
    // `synthetic` and so already returned `None` above; only an embed's
    // non-synthetic anchor row reaches here) is never walked through
    // `render::segment_geometry` below when it's actually showing
    // placeholder pixels — that would measure the row's ORDINARY
    // span-based content (its alt text), not the placeholder-cell layout
    // `render::image::row_cells` actually draws, and mouse coordinates
    // would disagree with what's on screen (plan gotcha: "mirror the
    // override into the geometry-only variant"). Reuses `row_cells`
    // directly rather than a second, independently-written cell walk — the
    // exact chokepoint `build_rows` itself calls through. Every placeholder
    // cell carries `buf_offset: -1` (no real buffer position of its own),
    // so a click anywhere on such a row resolves to the row's own
    // wrap-segment start rather than walking column-by-column.
    if let Some(image_ref) = display_ref.and_then(|r| r.image.clone())
        && image_ref.target.is_some()
        && let Some(cells) =
            crate::render::image::row_cells(app, doc, image_ref, doc.viewport.width)
    {
        return if (col as usize) < cells.len() {
            Some(row_start_offset(doc, view, wrap_row))
        } else {
            None
        };
    }

    offset_at_ordinary(doc, view, display_row, wrap_row, content, col)
}

/// The wrap segment `wrap_row`'s own first buffer position — what an image
/// row's click resolves to (plan WP9.S3), since none of its placeholder
/// cells carry a real one of their own to walk toward.
fn row_start_offset(doc: &Document, view: &ViewSnapshots, wrap_row: usize) -> usize {
    let content = doc.buffer.content();
    let syntax_point = view.wrap.wrap_to_syntax(
        content,
        WrapPoint {
            row: wrap_row,
            col: 0,
        },
    );
    let buffer_point = view.syntax.syntax_to_buffer(syntax_point);
    doc.buffer.line_col_to_offset(buffer_point)
}

/// The ordinary (non-image) row cell walk — unchanged from before WP9.S3,
/// split out so the image-row branch above can share this tail (the
/// past-the-last-cell fallback) without duplicating it.
fn offset_at_ordinary(
    doc: &Document,
    view: &ViewSnapshots,
    display_row: usize,
    wrap_row: usize,
    content: &str,
    col: u16,
) -> Option<usize> {
    // WP4.S4: the clicked row may carry a decoration prefix (heading icon /
    // bullet / quote bar / hr rule, `build_rows` prepends it before this
    // same `render::segment_geometry` content the cell walk below reads) —
    // subtract its width from `col` before walking so a click's column
    // still lines up with the CONTENT cells `segment_geometry` returns
    // (which carry no decor prefix of their own). Clamped at 0 rather than
    // subtracting past it: a click landing ON the decor prefix itself
    // degrades to column 0 of the content, i.e. the line's own first
    // content cell, per the fallback below.
    let decor_width = view
        .display
        .rows()
        .get(display_row)
        .map(crate::render::decor::decor_cell_width)
        .unwrap_or(0) as usize;
    let col = (col as usize).saturating_sub(decor_width) as u16;

    let mut acc = 0usize;
    let mut first_content_offset: Option<i64> = None;
    if let Some(seg) = view.wrap.segments().get(wrap_row) {
        for cell in render::segment_geometry(content, &seg.spans) {
            if first_content_offset.is_none() && cell.buf_offset >= 0 {
                first_content_offset = Some(cell.buf_offset);
            }
            let width = cell.width.max(1) as usize;
            if (col as usize) < acc + width {
                return Some(if cell.buf_offset >= 0 {
                    cell.buf_offset as usize
                } else {
                    // A click resolving onto a decorative cell (should only
                    // ever be a table's synthetic border padding, since the
                    // line-decoration prefix was already subtracted above)
                    // falls back to the row's own first REAL cell rather
                    // than jumping to document offset 0 — no test pinned
                    // the old `Some(0)` behaviour (plan Gotchas), and it
                    // silently sent an unrelated click to the document
                    // start.
                    first_content_offset.unwrap_or(0) as usize
                });
            }
            acc += width;
        }
    }

    let seg_len = view.wrap.segment_len_at(wrap_row);
    let syntax_point = view.wrap.wrap_to_syntax(
        content,
        WrapPoint {
            row: wrap_row,
            col: seg_len,
        },
    );
    let buffer_point = view.syntax.syntax_to_buffer(syntax_point);
    Some(doc.buffer.line_col_to_offset(buffer_point))
}

/// The visual column `offset` sits at — so a cursor a click plants keeps a
/// sensible `desired_col` for a later Up/Down arrow, the same convention
/// `commands::nav::update_horizontal` uses for every keyboard motion.
fn desired_col_at(doc: &Document, view: &ViewSnapshots, offset: usize) -> usize {
    let bp = doc.buffer.offset_to_line_col(offset);
    let sp = view.syntax.buffer_to_syntax(bp);
    let wp = view.wrap.syntax_to_wrap(sp);
    view.wrap.visual_col(doc.buffer.content(), wp.row, wp.col)
}
