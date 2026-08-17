//! The Cell model + blit: `DisplaySnapshot`
//! wrap rows -> `Vec<Vec<Cell>>` (Rendered spans via `cell_map`, Revealed
//! spans via byte arithmetic) -> overlays keyed on
//! `buf_offset` (cursor reverse-video, selection background, synthetic EOL
//! cursor cell) -> blit into `frame.buffer_mut()`.
//! The terminal cursor stays hidden (`term::Guard::new`); the caret drawn
//! here IS the cursor.
//!
//! Split for the 500-line budget: [`cell`] holds the `Cell` type and the
//! buffer-content -> `Cell` walk (`segment_cells`/`segment_geometry`),
//! [`blit`] holds the terminal-buffer write, [`code_bg`] holds the
//! code-region background rectangle, and [`overlay`] holds the
//! cursor/selection/highlight overlays `build_rows` below applies — all
//! re-exported here so `render::Cell`/`render::segment_cells`/`render::blit`
//! stay the paths every other module already calls through. [`rowbg`] is a
//! fourth, unrelated background pass: the left column's Explorer/Tabs
//! panes render straight to the ratatui `Buffer` rather than through this
//! module's `Cell` row model, so their row backgrounds go through their
//! own chokepoint instead.

mod blit;
mod cell;
mod code_bg;
pub(crate) mod decor;
mod diff;
pub mod filesearch;
pub mod image;
mod overlay;
pub mod rowbg;
pub mod search;
pub mod title;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, BorderType, Borders};

use rune_md::element::doc::ViewSnapshots;
use rune_merge::RegionKind;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::messages;
use crate::pane::Pane;

pub use blit::blit;
pub(crate) use cell::paint_range;
pub use cell::{Cell, segment_cells, segment_geometry, style_for};

/// Builds the visible `Vec<Vec<Cell>>` for `doc`'s viewport: the DISPLAY
/// rows (wrap rows plus synthesised table borders) in `[scroll_row,
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
pub fn build_rows(
    app: &App,
    doc: &Document,
    doc_id: Option<DocumentId>,
    view: &ViewSnapshots,
) -> Vec<Vec<Cell>> {
    let viewport = &doc.viewport;
    let content = doc.buffer.content();
    let mut rows: Vec<Vec<Cell>> = crate::viewport::visible_rows(view.display.rows(), viewport)
        .map(|row| {
            // A row carrying an `ImageRowRef` (set by
            // either the whole-document image producer or, for an embed,
            // `expand_images`) renders through the placeholder/info-card
            // override instead of the ordinary span-cell path below — its
            // spans are decorative placeholders with no real content to
            // walk. `row_cells` returns `None` for an embed row that isn't
            // currently showable as pixels (no Kitty) — that case falls
            // through to the ordinary path so the row's own alt-text span
            // (the `Rendered` emit) shows instead.
            if let Some(image_ref) = row.image.clone()
                && let Some(cells) = image::row_cells(app, doc, &image_ref, viewport.width)
            {
                return cells;
            }
            // The row's own decoration (heading icon / bullet /
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
    let spans = overlay::visible_byte_range(&rows).map_or_else(Vec::new, |range| {
        crate::highlight::visible_spans(doc, range)
    });
    overlay::apply_highlight_spans(&mut rows, &spans, &app.theme);

    // The search bar's live match highlight: painted AFTER
    // the token overlay (so a match's background sits under, not over, a
    // token's foreground) and BEFORE the cursor overlays just below (so
    // the caret/selection still wins where they land on a match). Guarded
    // on `doc_id` actually being the ACTIVE document — `build_rows` is
    // generic over `doc` (the messages pane renders its own read-only
    // `Document`, `doc_id: None`, through this same function), but
    // `App::search`'s matches are computed against the active document's
    // bytes only.
    if let Some(state) = app.search()
        && doc_id == Some(app.active)
    {
        for m in &state.matches {
            paint_range(&mut rows, m.clone(), app.theme.chrome.search_match_bg);
        }
    }

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

    if let Some(focus) = doc.reading_link_focus {
        apply_reading_link_focus(&mut rows, focus);
    }

    rows
}

fn apply_reading_link_focus(rows: &mut [Vec<Cell>], focus: rune_syntax::element::ByteRange) {
    for row in rows.iter_mut() {
        for cell in row.iter_mut() {
            if cell.buf_offset < 0 {
                continue;
            }
            if focus.contains(cell.buf_offset as usize) {
                cell.style = cell.style.add_modifier(ratatui::style::Modifier::REVERSED);
            }
        }
    }
}

// `apply_cursor_overlays`, `highlight_selection`, `place_caret` and
// `apply_highlight_spans` moved to `overlay.rs` (500-line budget) — `build_rows`
// above calls them through `overlay::`.

/// Blits `app.view`'s current snapshot into the editor rect, and every
/// other chrome rect `layout::geometry` computes (`draw`
/// itself no longer computes any split — it consumes `Geometry`, the one
/// chokepoint every rect comes from, so it can never disagree with
/// `App::relayout`'s or `explorer`/`opentabs`'s own idea of the same
/// rects).
///
/// Render order is load-bearing: the center
/// `Block` must paint before the editor rows are blitted (its border
/// spans the WHOLE `geo.center` rect, including where the editor sits one
/// cell in), and the breadcrumb overlay must run after both — it splices
/// directly onto the border row the `Block` already painted.
pub fn draw(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let geo = crate::layout::geometry(area, app);

    // `draw_left_pane` itself no-ops on `geo.left_block == None` (a
    // redundant guard used to re-check the same
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

    if let Some(bar_area) = geo.search_bar {
        search::draw(app, bar_area, frame);
    }

    if let Some(diff_left) = geo.diff_left {
        draw_diff_left(app, diff_left, frame);
    }

    if let Some(view) = &app.active_doc().view {
        let mut rows = build_rows(app, app.active_doc(), Some(app.active), view);
        if let Some(diff_view) = app.diff.as_ref()
            && diff_view.right == app.active
        {
            let layout = diff::layout(diff_view, &view.wrap);
            let right_scroll = app.active_doc().viewport.scroll_row.0;
            rows = if geo.diff_left.is_some() {
                diff::augment(
                    &rows,
                    &layout,
                    crate::diff_view::rows::Side::Right,
                    right_scroll,
                    geo.editor.height,
                    geo.editor.width,
                )
            } else {
                let left_scroll = diff_view.left.viewport.scroll_row.0;
                let left_rows = diff_view
                    .left
                    .view
                    .as_ref()
                    .map(|v| build_rows(app, &diff_view.left, None, v))
                    .unwrap_or_default();
                diff::augment_fold(
                    &rows,
                    &left_rows,
                    &layout,
                    right_scroll,
                    left_scroll,
                    app.theme.chrome.merge_theirs_bg,
                    geo.editor.height,
                    geo.editor.width,
                )
            };
            diff::paint_backgrounds(
                &mut rows,
                &diff_view.alignment,
                app.active_doc().buffer.content(),
                |r| r.right_lines.clone(),
                |k| matches!(k, RegionKind::Changed | RegionKind::RightOnly),
                app.theme.chrome.merge_ours_bg,
            );
            diff::paint_intraline(
                &mut rows,
                &diff_view.intraline_right,
                app.theme.chrome.diff_word_ours,
            );
        }
        blit(&rows, geo.editor, frame);
    } else {
        draw_pending(app.active_doc(), geo.editor, frame);
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

/// The pre-snapshot frame `draw` falls back to while `doc.view` is still
/// `None` — a large document's first display-pipeline compute runs on a
/// background `Cmd` (`runtime::bootstrap`'s large-document branch), and
/// nothing is ever representable as a PARTIAL `DisplaySnapshot` (issue #11:
/// `DisplaySnapshot::total_rows`/`wrap_to_display`/`display_to_wrap` clamp a
/// short prefix indistinguishably from a genuinely short document, which
/// would silently corrupt every scroll/caret/hit-test query built against
/// it). This reads straight from `Buffer`, unstyled, no wrap/emit pass, no
/// syntax highlighting — and bounded to `area.height` lines via `str::
/// lines().take(..)`, which stops walking the string the moment enough
/// lines are found, so drawing this frame costs the same tiny constant
/// regardless of how large the document is; the message pane (`runtime::
/// bootstrap`'s own call into `messages::info`) is the on-screen indicator
/// that the real content is still being prepared.
fn draw_diff_left(app: &App, area: Rect, frame: &mut Frame) {
    let Some(diff_view) = app.diff.as_ref() else {
        return;
    };
    let right_wrap = app
        .doc(diff_view.right)
        .and_then(|d| d.view.as_ref())
        .map(|v| &v.wrap);
    if let (Some(view), Some(right_wrap)) = (diff_view.left.view.as_ref(), right_wrap) {
        let native_rows = build_rows(app, &diff_view.left, None, view);
        let layout = diff::layout(diff_view, right_wrap);
        let scroll = diff_view.left.viewport.scroll_row.0;
        let mut rows = diff::augment(
            &native_rows,
            &layout,
            crate::diff_view::rows::Side::Left,
            scroll,
            area.height,
            area.width,
        );
        diff::paint_backgrounds(
            &mut rows,
            &diff_view.alignment,
            diff_view.left.buffer.content(),
            |r| r.left_lines.clone(),
            |k| matches!(k, RegionKind::Changed | RegionKind::LeftOnly),
            app.theme.chrome.merge_theirs_bg,
        );
        diff::paint_intraline(
            &mut rows,
            &diff_view.intraline_left,
            app.theme.chrome.diff_word_theirs,
        );
        blit(&rows, area, frame);
    }
    let divider = Rect::new(area.right(), area.y, 1, area.height);
    frame.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(app.theme.chrome.inactive_border),
        divider,
    );
}

fn draw_pending(doc: &Document, area: ratatui::layout::Rect, frame: &mut Frame) {
    let lines: Vec<ratatui::text::Line> = doc
        .buffer
        .content()
        .lines()
        .take(area.height as usize)
        .map(ratatui::text::Line::from)
        .collect();
    frame.render_widget(ratatui::widgets::Paragraph::new(lines), area);
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

    // The finder replaces only the Explorer's own content — the Tabs
    // section below it stays exactly as it always renders — so this is
    // checked first, ahead of every other title/content decision below.
    let filesearch_active = app.filesearch().is_some();

    // The Explorer can now be collapsed independently of the column
    // itself (its own vertical splitter dragged to the top): when it has
    // no rows to draw into, titling the block " Files " would claim a
    // pane that isn't there, so the block's title follows what's actually
    // showing instead of assuming the Explorer always is. A live type-to-
    // search (`App::explorer_find`) takes the next priority: the query is
    // the whole visible-feedback story the design calls for (no chord, no
    // mode indicator elsewhere on screen), so the block's own title is the
    // one place it can show without stealing an entry row.
    let title = if filesearch_active {
        " Open File ".to_string()
    } else if geo.explorer_inner.height == 0 {
        " Open ".to_string()
    } else if let Some(query) = app.explorer_find() {
        // Truncated to the block's own inner width (minus the two corner
        // cells) in terminal CELLS, not chars — a long query on a
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

    if filesearch_active {
        filesearch::draw(app, geo.explorer_inner, frame);
    } else if geo.explorer_inner.height > 0 {
        crate::explorer::draw(app, geo.explorer_inner, frame);
    }
    if let Some(divider) = geo.tabs_divider {
        crate::opentabs::draw_divider(app, divider, frame);
    }
    crate::opentabs::draw(app, geo.tabs_inner, frame);
}
