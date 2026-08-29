use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::{Cursor, CursorSet, CursorSpec};

use crate::app::App;
use crate::commands::mouse_hit::hit_test;
use crate::commands::nav::word_range_at;
use crate::commands::nav_line::line_range_incl_newline;
use crate::commands::nav_scroll;
use crate::commands::splitter;
use crate::diff_view::rows::{line_offset, right_line_for_left_line};
use crate::document::{Document, DocumentId};
use crate::explorer_mouse;
use crate::filesearch;
use crate::messages;
use crate::navigate;
use crate::opentabs;
use crate::pane::Pane;
use crate::pointer::{Drag, MouseButton, MouseInput, MouseKind};
use crate::runtime::Effects;

pub(crate) const WHEEL_ROWS: isize = 3;

pub fn handle(app: &mut App, input: MouseInput, effects: &mut Effects) {
    // A fresh press ends any latched gesture rather than being swallowed:
    // mode 1002 reports no hover, so a release lost to a focus change or an
    // out-of-window mouse-up has no second signal to recover from, and
    // swallowing input forever is worse than ending the drag one event
    // early.
    if let Some(Drag::Splitter { .. }) = app.pointer.drag {
        match input.kind {
            MouseKind::Drag(MouseButton::Left) => {
                splitter::drag(app, input, effects);
                return;
            }
            MouseKind::Up(MouseButton::Left) => {
                app.pointer.drag = None;
                return;
            }
            MouseKind::Down(_)
            | MouseKind::Up(_)
            | MouseKind::Drag(_)
            | MouseKind::ScrollUp
            | MouseKind::ScrollDown => app.pointer.drag = None,
        }
    }

    // A latched palette-row drag owns the pointer the same way: each tick
    // moves the selection to whatever row is under the pointer, and a
    // release just ends the gesture — the palette itself is never closed
    // or executed by a drag, only by a click or Enter.
    if let Some(Drag::Palette) = app.pointer.drag {
        match input.kind {
            MouseKind::Drag(MouseButton::Left) => {
                let area = app.frame_area();
                let geo = crate::layout::geometry(area, app);
                if let Some(rect) = geo.palette
                    && let Some(row) = palette_row_at(app, rect, input)
                {
                    crate::palette::keys::drag_hover(app, row);
                }
                return;
            }
            MouseKind::Up(MouseButton::Left) => {
                app.pointer.drag = None;
                return;
            }
            MouseKind::Down(_)
            | MouseKind::Up(_)
            | MouseKind::Drag(_)
            | MouseKind::ScrollUp
            | MouseKind::ScrollDown => app.pointer.drag = None,
        }
    }

    // A latched text-selection drag owns the pointer the same way, and for
    // the same reason: routed by the gesture's OWN pane, not by whichever
    // rect the pointer currently sits over, so a drag that has wandered
    // outside its origin pane (or a release out past the frame edge) still
    // reaches the document it began on instead of being dropped silently.
    if let Some(Drag::Text { anchor, pane }) = app.pointer.drag {
        match input.kind {
            MouseKind::Drag(MouseButton::Left) => {
                match pane {
                    Pane::Editor => handle_left_drag(app, anchor, input),
                    Pane::Messages => messages::mouse(app, input, effects),
                    Pane::Explorer | Pane::Tabs | Pane::Title => {}
                }
                return;
            }
            MouseKind::Up(MouseButton::Left) => {
                app.pointer.drag = None;
                if pane == Pane::Messages {
                    messages::mouse(app, input, effects);
                }
                return;
            }
            MouseKind::Down(_)
            | MouseKind::Up(_)
            | MouseKind::Drag(_)
            | MouseKind::ScrollUp
            | MouseKind::ScrollDown => app.pointer.drag = None,
        }
    }

    if matches!(input.kind, MouseKind::Down(MouseButton::Left)) && splitter::begin(app, input) {
        return;
    }

    let area = app.frame_area();
    let geo = crate::layout::geometry(area, app);

    if matches!(input.kind, MouseKind::Down(MouseButton::Left)) && app.filesearch().is_some() {
        handle_filesearch_click(app, geo.explorer_inner, input, effects);
        return;
    }

    if matches!(input.kind, MouseKind::Down(MouseButton::Left)) && app.projectsearch().is_some() {
        handle_projectsearch_click(app, geo.explorer_inner, input, effects);
        return;
    }

    if matches!(input.kind, MouseKind::Down(MouseButton::Left)) && app.palette().is_some() {
        let inside = geo.palette.is_some_and(|rect| {
            rect.contains(ratatui::layout::Position::new(input.column, input.row))
        });
        match (inside, geo.palette) {
            (true, Some(rect)) => handle_palette_down(app, rect, input, effects),
            _ => crate::palette::close(app),
        }
        return;
    }

    // Only inside the palette's own rect: a wheel tick elsewhere while the
    // palette happens to be open keeps its ordinary fallthrough behavior.
    if let (MouseKind::ScrollUp | MouseKind::ScrollDown, Some(rect)) = (input.kind, geo.palette)
        && rect.contains(ratatui::layout::Position::new(input.column, input.row))
    {
        let delta = if matches!(input.kind, MouseKind::ScrollUp) {
            -WHEEL_ROWS
        } else {
            WHEEL_ROWS
        };
        crate::palette::keys::nav_move(app, delta);
        return;
    }

    if let (Some(diff_left), MouseKind::Down(MouseButton::Left)) = (geo.diff_left, input.kind)
        && diff_left.contains(ratatui::layout::Position::new(input.column, input.row))
    {
        handle_diff_left_click(app, diff_left, input, effects);
        return;
    }

    match geo.pane_at(input.column, input.row) {
        Some(Pane::Messages) => messages::mouse(app, input, effects),
        // The finder paints over the Explorer's own rect while it is open,
        // so a wheel tick there must move what the user can actually see.
        Some(Pane::Explorer) if app.filesearch().is_some() => {
            filesearch::mouse(app, input, effects)
        }
        Some(Pane::Explorer) if app.projectsearch().is_some() => {
            crate::projectsearch::mouse(app, input)
        }
        Some(Pane::Explorer) => explorer_mouse::mouse(app, input, effects),
        Some(Pane::Tabs) => opentabs::mouse::mouse(app, input, effects),
        Some(Pane::Editor) => {
            let col = input.column.saturating_sub(geo.editor.x);
            let row = input.row.saturating_sub(geo.editor.y);
            match input.kind {
                MouseKind::ScrollUp => nav_scroll::scroll_lines(app.active_doc_mut(), -WHEEL_ROWS),
                MouseKind::ScrollDown => nav_scroll::scroll_lines(app.active_doc_mut(), WHEEL_ROWS),
                MouseKind::Down(MouseButton::Left) => {
                    handle_left_down(app, input, col, row, effects);
                }
                MouseKind::Down(MouseButton::Right | MouseButton::Middle)
                | MouseKind::Up(_)
                | MouseKind::Drag(_) => {}
            }
        }
        Some(Pane::Title) | None => {}
    }
}

fn handle_filesearch_click(
    app: &mut App,
    rect: ratatui::layout::Rect,
    input: MouseInput,
    effects: &mut Effects,
) {
    let point = ratatui::layout::Position::new(input.column, input.row);
    if !rect.contains(point) {
        filesearch::cancel(app, effects);
        return;
    }
    let row_in_area = input.row.saturating_sub(rect.y);
    let Some(visible_row) = row_in_area.checked_sub(1) else {
        return;
    };
    filesearch::click_row(app, visible_row as usize, effects);
}

fn handle_projectsearch_click(
    app: &mut App,
    rect: ratatui::layout::Rect,
    input: MouseInput,
    effects: &mut Effects,
) {
    let point = ratatui::layout::Position::new(input.column, input.row);
    if !rect.contains(point) {
        crate::projectsearch::cancel(app, effects);
        return;
    }
    let row_in_area = input.row.saturating_sub(rect.y);
    let Some(visible_row) = row_in_area.checked_sub(1) else {
        return;
    };
    crate::projectsearch::click_row(app, visible_row as usize);
}

fn handle_palette_down(
    app: &mut App,
    rect: ratatui::layout::Rect,
    input: MouseInput,
    effects: &mut Effects,
) {
    app.pointer.drag = Some(Drag::Palette);
    if let Some(row) = palette_row_at(app, rect, input) {
        crate::palette::keys::click_row(app, row, effects);
    }
}

/// Resolves an on-screen point inside the palette's bordered `rect` to an
/// absolute row index in `state.rows`/`state.arg_rows` — `None` for the
/// border, the query bar, an open refusal line, the recents separator, or
/// blank space past the last row.
fn palette_row_at(app: &App, rect: ratatui::layout::Rect, input: MouseInput) -> Option<usize> {
    let state = app.palette()?;
    let local_row = input.row.checked_sub(rect.y)?;
    if local_row == 0 {
        return None;
    }
    let inner_row = local_row - 1;
    if inner_row >= rect.height.saturating_sub(2) {
        return None;
    }
    let chrome = crate::palette::content_rows(state).saturating_sub(2);
    let row_in_window = usize::from(inner_row.checked_sub(chrome)?);
    let height = crate::palette::row_capacity(app);
    let window = state.nav.window(state.active_len(), height);
    let absolute = window.start.checked_add(row_in_window)?;
    (absolute < window.end).then_some(absolute)
}

fn handle_left_down(app: &mut App, input: MouseInput, col: u16, row: u16, effects: &mut Effects) {
    let departed = crate::navhistory::departure_origin(app);
    app.set_focus_pane(Pane::Editor, effects);
    if app.focus() != Pane::Editor {
        return;
    }
    crate::navhistory::record_departure_if_moved(app, departed);

    let Some((offset, desired_col)) = hit_test(app, app.active_doc(), row, col) else {
        return;
    };

    if input.ctrl {
        // Never registers toward the click-aggregation run, so a ctrl-click
        // can't accidentally chain into a double/triple-click select, and a
        // plain double-click right after it still starts its own fresh run.
        // ⌘+click is unavailable here: the SGR mouse protocol encodes only
        // shift/alt/ctrl, never Super.
        let doc = app.active_doc_mut();
        doc.cursors = CursorSet::new_from_specs(&[CursorSpec {
            position: BufferOffset(offset),
            anchor: BufferOffset(offset),
            desired_col: VisualCol(desired_col),
        }]);
        app.pointer.drag = None;
        navigate::follow(app, effects);
        return;
    }

    let now = app.clock.now();
    let count = app.pointer.register_click(now, input.column, input.row);

    if input.alt {
        let doc = app.active_doc_mut();
        doc.cursors = doc.cursors.add(CursorSpec {
            position: BufferOffset(offset),
            anchor: BufferOffset(offset),
            desired_col: VisualCol(desired_col),
        });
        app.pointer.drag = None;
        return;
    }

    if input.shift {
        // Extends the primary cursor's existing anchor to the click point,
        // collapsing any other cursor to it.
        let doc = app.active_doc_mut();
        let anchor = doc.cursors.primary().anchor;
        let id = doc.cursors.primary().id;
        let extended = Cursor {
            position: BufferOffset(offset),
            anchor,
            desired_col: VisualCol(desired_col),
            id,
        };
        doc.cursors = CursorSet::new_from(&[extended]);
        app.pointer.drag = Some(Drag::Text {
            anchor: anchor.get(),
            pane: Pane::Editor,
        });
        return;
    }

    let doc = app.active_doc_mut();
    if place_click_cursor(doc, offset, desired_col, count) {
        app.pointer.drag = Some(Drag::Text {
            anchor: offset,
            pane: Pane::Editor,
        });
    } else {
        app.pointer.drag = None;
    }
}

fn handle_diff_left_click(
    app: &mut App,
    diff_left: ratatui::layout::Rect,
    input: MouseInput,
    effects: &mut Effects,
) {
    app.set_focus_pane(Pane::Editor, effects);
    let col = input.column.saturating_sub(diff_left.x);
    let row = input.row.saturating_sub(diff_left.y);
    let Some((right, target_line)) = diff_left_click_target(app, row, col) else {
        return;
    };
    let Some(doc) = app.doc_mut(right) else {
        return;
    };
    let offset = line_offset(&doc.buffer, target_line);
    doc.cursors = CursorSet::new(offset);
    nav_scroll::scroll_to_byte_offset(doc, offset);
}

fn diff_left_click_target(app: &App, row: u16, col: u16) -> Option<(DocumentId, usize)> {
    let diff = app.diff.as_ref()?;
    let (offset, _) = hit_test(app, &diff.left, row, col)?;
    let left_line = diff.left.buffer.offset_to_line_col(offset).line;
    let right_line = right_line_for_left_line(&diff.alignment, left_line);
    Some((diff.right, right_line))
}

pub(crate) fn select_range(doc: &mut Document, start: usize, end: usize) {
    let id = doc.cursors.primary().id;
    let selected = Cursor {
        position: BufferOffset(end),
        anchor: BufferOffset(start),
        desired_col: VisualCol(0),
        id,
    };
    doc.cursors = CursorSet::new_from(&[selected]);
}

pub(crate) fn place_click_cursor(
    doc: &mut Document,
    offset: usize,
    desired_col: usize,
    count: u8,
) -> bool {
    match count {
        1 => {
            doc.cursors = CursorSet::new_from_specs(&[CursorSpec {
                position: BufferOffset(offset),
                anchor: BufferOffset(offset),
                desired_col: VisualCol(desired_col),
            }]);
            true
        }
        2 => {
            let (start, end) = word_range_at(&doc.buffer, offset);
            select_range(doc, start, end);
            false
        }
        _ => {
            let (start, end) = line_range_incl_newline(&doc.buffer, offset);
            select_range(doc, start, end);
            false
        }
    }
}

pub(crate) fn extend_drag_cursor(
    doc: &mut Document,
    anchor: usize,
    offset: usize,
    desired_col: usize,
) {
    let id = doc.cursors.primary().id;
    let extended = Cursor {
        position: BufferOffset(offset),
        anchor: BufferOffset(anchor),
        desired_col: VisualCol(desired_col),
        id,
    };
    doc.cursors = CursorSet::new_from(&[extended]);
}

fn handle_left_drag(app: &mut App, anchor: usize, input: MouseInput) {
    let area = app.frame_area();
    let geo = crate::layout::geometry(area, app);
    let editor = geo.editor;
    let point = ratatui::layout::Position::new(input.column, input.row);
    if !editor.contains(point) {
        return;
    }
    let col = input.column.saturating_sub(editor.x);
    let row = input.row.saturating_sub(editor.y);
    let Some((offset, desired_col)) = hit_test(app, app.active_doc(), row, col) else {
        return;
    };
    let doc = app.active_doc_mut();
    extend_drag_cursor(doc, anchor, offset, desired_col);
}

#[cfg(test)]
#[path = "mouse_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "palette_mouse_tests.rs"]
mod palette_mouse_tests;
