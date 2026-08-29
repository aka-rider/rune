mod blit;
mod bracket;
mod cell;
mod code_bg;
pub(crate) mod decor;
mod diff;
pub mod filesearch;
pub(crate) mod fuzzyspan;
pub mod image;
mod overlay;
pub mod palette;
pub mod projectsearch;
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
pub use cell::{Cell, segment_cells, segment_geometry, style_for};
pub(crate) use cell::{paint_range, push_grapheme_cells};

// Builds the visible `Vec<Vec<Cell>>` for `doc`'s viewport: DISPLAY rows
// in `[scroll_row, scroll_row + height)`, with cursor/selection overlays
// applied. Generic over `doc` so the messages pane can render its own
// read-only `Document` through the identical row-space pipeline the
// editor uses, which mouse hit-testing depends on.
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
            if let Some(image_ref) = row.image.clone()
                && let Some(cells) = image::row_cells(app, doc, &image_ref, viewport.width)
            {
                return cells;
            }
            let mut cells = decor::decor_row_cells(&app.theme, row);
            cells.extend(segment_cells(&app.theme, content, &row.spans));
            cells
        })
        .collect();

    // The code region background is a rectangle painted before token
    // colour, driven off the display snapshot's own `code_regions` —
    // never `doc.highlight` — so no highlight reply racing a render can
    // make two frames of the same document disagree on where code starts.
    code_bg::paint_code_background(
        &mut rows,
        view,
        viewport.scroll_row,
        viewport.width,
        app.theme.chrome.code_bg,
    );

    // The token overlay paints BEFORE the cursor overlays below, so a
    // selection background or the caret's reverse-video always wins over
    // a token's foreground. Queried fresh each frame, scoped to exactly
    // the visible byte window, so cost never grows with document size.
    let spans = overlay::visible_byte_range(&rows).map_or_else(Vec::new, |range| {
        crate::highlight::visible_spans(doc, range)
    });
    overlay::apply_highlight_spans(&mut rows, &spans, &app.theme);

    bracket::apply_bracket_match(&mut rows, doc, &app.theme);

    // Painted AFTER the token overlay (so a match's background sits under
    // a token's foreground) and BEFORE the cursor overlays (so the
    // caret/selection still wins). `build_rows` is generic over `doc` (the
    // messages pane renders through it too), but `app.search()`'s matches
    // only ever apply to the active document.
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
            let Some(offset) = cell.buf_offset else {
                continue;
            };
            if focus.contains(offset as usize) {
                cell.style = cell.style.add_modifier(ratatui::style::Modifier::REVERSED);
            }
        }
    }
}

// Render order is load-bearing: the center `Block` must paint before the
// editor rows are blitted (its border spans the whole `geo.center` rect,
// including where the editor sits one cell in), and the breadcrumb
// overlay must run after both — it splices directly onto the border row
// the `Block` already painted.
pub fn draw(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let geo = crate::layout::geometry(area, app);

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
        match geo.diff_left {
            Some(diff_left) => {
                let left_title = Rect::new(title_area.x, title_area.y, diff_left.width, 1);
                let right_x = geo.editor.x;
                let right_title = Rect::new(
                    right_x,
                    title_area.y,
                    title_area.right().saturating_sub(right_x),
                    1,
                );
                let left_name = app
                    .diff
                    .as_ref()
                    .map_or("", |d| d.left.file_name())
                    .to_string();
                title::draw_left(&left_name, left_title, &app.theme, frame);
                title::draw(app, right_title, frame);
            }
            None => title::draw(app, title_area, frame),
        }
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

    if let Some(messages_area) = geo.messages {
        messages::draw(app, messages_area, frame);
    }
    crate::footer::draw(app, geo.footer, frame);

    if let Some(palette_area) = geo.palette {
        palette::draw(app, palette_area, frame);
    }
}

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
    } else {
        blit(&blank_rows(area.width, area.height), area, frame);
    }
    let divider = Rect::new(area.right(), area.y, 1, area.height);
    frame.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(app.theme.chrome.inactive_border),
        divider,
    );
}

fn blank_rows(width: u16, height: u16) -> Vec<Vec<Cell>> {
    let blank = Cell {
        text: " ".into(),
        width: 1,
        style: ratatui::style::Style::default(),
        buf_offset: None,
    };
    (0..height)
        .map(|_| vec![blank.clone(); width as usize])
        .collect()
}

// `doc.view` is still `None` while a large document's first
// display-pipeline compute runs in the background: no prefix of a
// DisplaySnapshot is safely representable (a clamped partial would look
// identical to a genuinely short document to every scroll/caret/hit-test
// query built against it). This instead reads straight from `Buffer`,
// unstyled, bounded to `area.height` lines.
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

// The left column: ONE titled, bordered block holding both the Explorer
// and Open Tabs panes, with Tabs introduced by an in-block divider row
// rather than a second border. Render order is load-bearing: the block
// paints first — its border spans the whole rect — then the Explorer
// rows, the divider, and the tab rows go on top.
fn draw_left_pane(app: &App, geo: &crate::layout::Geometry, frame: &mut Frame) {
    let Some(left_area) = geo.left_block else {
        return;
    };

    let filesearch_active = app.filesearch().is_some();
    let projectsearch_active = app.projectsearch().is_some();

    let title = if filesearch_active {
        " Open File ".to_string()
    } else if projectsearch_active {
        " Search Project ".to_string()
    } else if geo.explorer_inner.height == 0 {
        " Open ".to_string()
    } else if let Some(query) = app.explorer_find() {
        // Truncated to the block's own inner width in terminal CELLS, not
        // chars, so a long query on a narrow column can't overrun the
        // border.
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
    } else if projectsearch_active {
        projectsearch::draw(app, geo.explorer_inner, frame);
    } else if geo.explorer_inner.height > 0 {
        crate::explorer::draw(app, geo.explorer_inner, frame);
    }
    if let Some(divider) = geo.tabs_divider {
        crate::opentabs::draw_divider(app, divider, frame);
    }
    crate::opentabs::draw(app, geo.tabs_inner, frame);
}
