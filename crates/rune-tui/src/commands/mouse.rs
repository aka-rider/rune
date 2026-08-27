//! Mouse gesture dispatch: click positions the caret,
//! alt-click adds a cursor, shift-click extends the selection, double-click
//! selects the word, triple-click selects the whole logical line (including
//! every wrapped row), plain click-drag extends a selection, and the wheel
//! scrolls 3 rows. `app::update` routes `Msg::Mouse` here directly, exactly
//! like `app::handle_key` routes a resolved `Command` to `commands::nav`.

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

/// The mouse wheel's step: vim, neovim's `mousescroll=ver:3`, and Helix's
/// `scroll-lines = 3` all converge on this number.
pub(crate) const WHEEL_ROWS: isize = 3;

/// Routes one `Msg::Mouse`. Takes `effects` because a ctrl-click may follow
/// an external link, which needs an `OpenExternal` `Cmd`.
pub fn handle(app: &mut App, input: MouseInput, effects: &mut Effects) {
    // A splitter drag owns the pointer until the button comes up: it
    // routinely leaves every rect mid-gesture, so this is decided before
    // the pane dispatch below would otherwise drop the event. A fresh
    // press ends any latched gesture rather than being swallowed — mode
    // 1002 reports no hover, so a release lost to a focus change or an
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
            // Anything else (a wheel tick, a right-button press, a fresh
            // left press) means the gesture is over. Clear it and let the
            // event fall through to its normal handling below.
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

    if matches!(input.kind, MouseKind::Down(MouseButton::Left)) && app.filesearch().is_some() {
        filesearch::cancel(app, effects);
        return;
    }

    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let geo = crate::layout::geometry(area, app);

    if matches!(input.kind, MouseKind::Down(MouseButton::Left)) && app.palette().is_some() {
        let inside = geo.palette.is_some_and(|rect| {
            rect.contains(ratatui::layout::Position::new(input.column, input.row))
        });
        if !inside {
            crate::palette::close(app);
        }
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
        // Ctrl-click: place the caret at the hit-tested offset and follow
        // whatever link sits there — never registers toward
        // the click-aggregation run (`PointerState::register_click` isn't
        // called on this branch), so a ctrl-click can never accidentally
        // chain into a double/triple-click select, and a plain double-click
        // right after it still starts its own fresh run. ⌘+click is
        // unavailable here: the SGR mouse protocol encodes only shift/alt/
        // ctrl, never Super.
        let doc = app.active_doc_mut();
        doc.cursors = CursorSet::new_from_specs(&[CursorSpec {
            position: offset,
            anchor: offset,
            desired_col,
        }]);
        app.pointer.drag = None;
        navigate::follow(app, effects);
        return;
    }

    let now = app.clock.now();
    let count = app.pointer.register_click(now, input.column, input.row);

    if input.alt {
        // Alt-click: add a cursor, never disturbing the existing set.
        let doc = app.active_doc_mut();
        doc.cursors = doc.cursors.add(CursorSpec {
            position: offset,
            anchor: offset,
            desired_col,
        });
        app.pointer.drag = None;
        return;
    }

    if input.shift {
        // Shift-click: extend the PRIMARY cursor's existing anchor to the
        // click point, collapsing any other cursor (matches most editors:
        // shift-click is a single-selection gesture).
        let doc = app.active_doc_mut();
        let anchor = doc.cursors.primary().anchor;
        let id = doc.cursors.primary().id;
        let extended = Cursor {
            position: offset,
            anchor,
            desired_col,
            id,
        };
        doc.cursors = CursorSet::new_from(&[extended]);
        app.pointer.drag = Some(Drag::Text {
            anchor,
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
        position: end,
        anchor: start,
        desired_col: 0,
        id,
    };
    doc.cursors = CursorSet::new_from(&[selected]);
}

/// The click-count -> cursor shape every left-mouse-down handler shares (the
/// editor's own `handle_left_down` and the messages pane's `mouse_down`): a
/// plain click places the caret, a double-click selects the word under it,
/// three or more selects the whole logical line. Returns whether the caller
/// should latch a `Drag::Text` — only a plain single click does; the
/// multi-click cases already produced a full selection. The editor-only
/// modifier gestures (ctrl/alt/shift-click) stay in `handle_left_down`
/// itself, since the messages pane has no use for them.
pub(crate) fn place_click_cursor(
    doc: &mut Document,
    offset: usize,
    desired_col: usize,
    count: u8,
) -> bool {
    match count {
        1 => {
            doc.cursors = CursorSet::new_from_specs(&[CursorSpec {
                position: offset,
                anchor: offset,
                desired_col,
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

/// Extends `doc`'s selection for a latched `Drag::Text { anchor, .. }` to
/// `offset` — the drag half of the shape [`place_click_cursor`] shares:
/// both the editor's `handle_left_drag` and the messages pane's
/// `mouse_drag` reach this once they've hit-tested a buffer offset.
pub(crate) fn extend_drag_cursor(
    doc: &mut Document,
    anchor: usize,
    offset: usize,
    desired_col: usize,
) {
    let id = doc.cursors.primary().id;
    let extended = Cursor {
        position: offset,
        anchor,
        desired_col,
        id,
    };
    doc.cursors = CursorSet::new_from(&[extended]);
}

/// Extends the editor's selection for a latched `Drag::Text { pane: Editor,
/// .. }` — called directly by the top-of-`handle` latched-gesture branch, so
/// it recomputes the editor rect itself rather than trusting rect-relative
/// coordinates a caller elsewhere might compute against the wrong rect. A
/// pointer that has wandered outside the editor mid-drag is a no-op (the
/// selection simply stops extending until it re-enters).
fn handle_left_drag(app: &mut App, anchor: usize, input: MouseInput) {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let geo = crate::layout::geometry(area, app);
    let editor = geo.editor;
    // Containment against the editor's OWN rect, never `pane_at` — `pane_at`
    // deliberately classifies every `diff_left` column as `Pane::Editor` too
    // (`Geometry::pane_at`'s own docs), so it can't tell a drag that has
    // wandered into the diff-left pane from one still inside the editor,
    // and `input.column`/`input.row` can then sit left of/above `editor.x`/
    // `editor.y` — exactly the case `saturating_sub` below guards as a
    // second line of defense.
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
