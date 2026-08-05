//! The Cell model + blit (plan Context, "Cell model"): `DisplaySnapshot`
//! wrap rows -> `Vec<Vec<Cell>>` (Rendered spans via `cell_map`, Revealed
//! spans via byte arithmetic — port of Go's cell builder) -> overlays keyed on
//! `buf_offset` (cursor reverse-video, selection background, synthetic EOL
//! cursor cell, Go parity) -> blit into `frame.buffer_mut()`.
//! The terminal cursor stays hidden (`term::Guard::new`); the caret drawn
//! here IS the cursor, Go parity.
//!
//! Split for the §1.6 budget: [`cell`] holds the `Cell` type and the
//! buffer-content -> `Cell` walk (`segment_cells`/`segment_geometry`),
//! [`blit`] holds the terminal-buffer write, [`code_bg`] holds the
//! code-region background rectangle, and [`overlay`] holds the
//! cursor/selection/highlight overlays `build_rows` below applies — all
//! re-exported here so `render::Cell`/`render::segment_cells`/`render::blit`
//! stay the paths every other module already calls through.

mod blit;
mod cell;
mod code_bg;
pub(crate) mod decor;
pub mod image;
mod overlay;
pub mod title;

use ratatui::Frame;
use ratatui::widgets::{Block, BorderType};

use rune_md::element::doc::ViewSnapshots;

use crate::app::App;
use crate::document::Document;
use crate::messages;
use crate::pane::Pane;

pub use blit::blit;
pub use cell::{Cell, segment_cells, segment_geometry, style_for};

/// Builds the visible `Vec<Vec<Cell>>` for `doc`'s viewport: the DISPLAY
/// rows (WP3: wrap rows plus synthesised table borders) in `[scroll_row,
/// scroll_row + height)`, with cursor/selection overlays applied. Generic
/// over `doc` so the messages pane can render its own read-only `Document`
/// through the identical pipeline the editor uses —
/// required for mouse hit-testing to ever land correctly, since `render`'s
/// row space and a bespoke row walk (the old `banner::build_rows`) are
/// NOT the same space. `viewport.scroll_row` is a DISPLAY row index —
/// `Document::scroll_to_cursor` is its single writer and converts through
/// `DisplaySnapshot::wrap_to_display` before ever assigning it (see that
/// function's docs). Merge mode's ours/theirs/marker overlay is NOT painted
/// here — it is the active editor document's own content, so `render::draw`
/// applies it itself, at the one call site that actually knows `doc` is the
/// active document.
pub fn build_rows(app: &App, doc: &Document, view: &ViewSnapshots) -> Vec<Vec<Cell>> {
    let viewport = &doc.viewport;
    let content = doc.buffer.content();
    let mut rows: Vec<Vec<Cell>> = crate::viewport::visible_rows(view.display.rows(), viewport)
        .map(|row| {
            // WP4.S11/WP9.S1: a row carrying an `ImageRowRef` (set by
            // either the whole-document image producer or, for an embed,
            // `expand_images`) renders through the placeholder/info-card
            // override instead of the ordinary span-cell path below — its
            // spans are decorative placeholders with no real content to
            // walk. `row_cells` returns `None` for an embed row that isn't
            // currently showable as pixels (no Kitty) — that case falls
            // through to the ordinary path so the row's own alt-text span
            // (WP7's `Rendered` emit) shows instead.
            if let Some(image_ref) = row.image.clone()
                && let Some(cells) = image::row_cells(app, doc, image_ref, viewport.width)
            {
                return cells;
            }
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

    // A code region's background is a RECTANGLE, painted before any token
    // colour so a foreground lands on top of it rather than being
    // overwritten by it. It is deliberately driven off the display
    // snapshot's own `code_regions` (the one definition of code, shared with
    // the highlight scheduler, computed once per document change) — never
    // `doc.highlight` — so no highlight reply can reflow a row and two
    // message-free renders agree.
    code_bg::paint_code_background(
        &mut rows,
        view,
        viewport.scroll_row,
        viewport.width,
        app.theme.chrome.code_bg,
    );

    // The token overlay paints BEFORE the cursor overlays below, so a
    // selection background or the caret's reverse-video always wins over a
    // token's foreground. Every region with a retained tree is queried fresh
    // each frame, scoped to exactly the bytes just rendered:
    // `visible_byte_range` derives the same window the span overlay itself
    // scans, so the query never does work outside it. This is the whole
    // parse -> render seam — a fence and a whole file reach it identically.
    let spans = match overlay::visible_byte_range(&rows) {
        Some(range) => crate::highlight::visible_spans(doc, range),
        None => Vec::new(),
    };
    overlay::apply_highlight_spans(&mut rows, &spans, &app.theme);

    overlay::apply_cursor_overlays(
        overlay::OverlayGates {
            caret: doc.has_insertion_point(),
            selection: doc.shows_selection(),
        },
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
        let mut rows = build_rows(app, app.active_doc(), view);
        // Merge mode's ours/theirs/marker backgrounds paint LAST, on top of
        // every overlay `build_rows` already applied — the working form's
        // markers and framed spans are merge mode's own content, not
        // markdown or code to be highlighted underneath them. Painted here
        // rather than inside `build_rows`: this is the one call site that
        // actually knows `doc` is the active
        // document, so a generic `build_rows` (the messages pane's own
        // caller included) can never mistakenly paint a merge overlay onto
        // some other document.
        crate::merge::paint::paint(&mut rows, &app.merge, app.active, &app.theme);
        blit(&rows, geo.editor, frame);
    }

    if geo.center_bordered {
        crate::breadcrumb::overlay(app, geo.center, app.focus() == Pane::Editor, frame);
    }

    // The one messages-pane delegation — all of its own cell building
    // lives in `messages::render`, never here.
    if let Some(messages_area) = geo.messages {
        messages::draw(app, messages_area, frame);
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
    // showing instead of assuming the Explorer always is. A live type-to-
    // search (`Explorer::search`) takes the next priority: the query is
    // the whole visible-feedback story the design calls for (no chord, no
    // mode indicator elsewhere on screen), so the block's own title is the
    // one place it can show without stealing an entry row.
    let title = if geo.explorer_inner.height == 0 {
        " Open ".to_string()
    } else if let Some(query) = &app.explorer.search {
        // Truncated to the block's own inner width (minus the two corner
        // cells) in terminal CELLS (§1.5), not chars — a long query on a
        // narrow column must not overrun the border.
        let budget = (left_area.width as usize).saturating_sub(2);
        let raw = format!(" Search: {query} ");
        crate::width::truncate_tail_to_width(&raw, budget)
    } else {
        " Files ".to_string()
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
