//! Mouse gesture dispatch (plan WP7.S6): click positions the caret,
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

/// The mouse wheel's step (plan WP7.S6: "wheel scrolls 3 rows" — vim,
/// neovim's `mousescroll=ver:3`, and Helix's `scroll-lines = 3` all
/// converge on this number).
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
        // whatever link sits there (plan WP5.S8) — never registers toward
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
/// selection simply stops extending until it re-enters), matching the
/// pre-WP3 behaviour this replaces.
fn handle_left_drag(app: &mut App, anchor: usize, input: MouseInput) {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let geo = crate::layout::geometry(area, app);
    if geo.pane_at(input.column, input.row) != Some(Pane::Editor) {
        return;
    }
    let editor = geo.editor;
    let col = input.column - editor.x;
    let row = input.row - editor.y;
    let Some((offset, desired_col)) = hit_test(app, app.active_doc(), row, col) else {
        return;
    };
    let doc = app.active_doc_mut();
    extend_drag_cursor(doc, anchor, offset, desired_col);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::pointer::ManualClock;
    use crate::runtime::Msg;
    use rune_core::buffer::Buffer;
    use rune_core::coords::DisplayRow;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str, width: u16, height: u16) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        app.clock = Arc::new(ManualClock::new());
        app.frame_width = width;
        app.frame_height = height + 1; // + footer row
        app.sync_view();
        app
    }

    /// `col`/`row` are relative to the EDITOR rect (what a gesture actually
    /// hit-tests against) — translated here to the absolute frame
    /// coordinates a real `MouseInput` carries, through the same
    /// `layout::geometry` call `commands::mouse::handle` itself uses, so a
    /// test can never silently click the border/title row instead of the
    /// editor content.
    fn editor_origin(app: &App) -> (u16, u16) {
        let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
        let editor = crate::layout::geometry(area, app).editor;
        (editor.x, editor.y)
    }

    #[derive(Clone, Copy, Default)]
    struct Modifiers {
        shift: bool,
        alt: bool,
    }

    fn click(app: &mut App, kind: MouseKind, col: u16, row: u16) {
        click_modified(app, kind, col, row, Modifiers::default());
    }

    fn click_modified(app: &mut App, kind: MouseKind, col: u16, row: u16, modifiers: Modifiers) {
        let (ox, oy) = editor_origin(app);
        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            app,
            Msg::Mouse(MouseInput {
                kind,
                column: ox + col,
                row: oy + row,
                shift: modifiers.shift,
                alt: modifiers.alt,
                ctrl: false,
            }),
            &mut effects,
        );
    }

    #[test]
    fn plain_click_positions_the_caret() {
        let mut app = app_with("hello world\n", 40, 10);
        click(&mut app, MouseKind::Down(MouseButton::Left), 6, 0);
        assert_eq!(app.active_doc().cursors.primary().position, 6);
        assert!(!app.active_doc().cursors.primary().has_selection());
    }

    #[test]
    fn double_click_selects_the_word() {
        let mut app = app_with("hello world\n", 40, 10);
        click(&mut app, MouseKind::Down(MouseButton::Left), 2, 0);
        click(&mut app, MouseKind::Down(MouseButton::Left), 2, 0);
        let c = app.active_doc().cursors.primary();
        assert_eq!(c.selection_range(), (0, 5)); // "hello"
    }

    #[test]
    fn triple_click_on_a_wrapped_line_selects_the_whole_logical_line() {
        // One long logical line, wrapped across several rows at width 10.
        let content = "aaaaaaaaaa bbbbbbbbbb cccccccccc\nsecond\n";
        let mut app = app_with(content, 10, 20);
        // The click lands on the SECOND wrapped row of the first logical
        // line (row 1), not its first — the gesture must still select the
        // whole logical line, every wrapped row included.
        click(&mut app, MouseKind::Down(MouseButton::Left), 2, 1);
        click(&mut app, MouseKind::Down(MouseButton::Left), 2, 1);
        click(&mut app, MouseKind::Down(MouseButton::Left), 2, 1);
        let c = app.active_doc().cursors.primary();
        let expected_end = "aaaaaaaaaa bbbbbbbbbb cccccccccc\n".len();
        assert_eq!(c.selection_range(), (0, expected_end));
    }

    #[test]
    fn wheel_scrolls_three_rows_without_moving_the_cursor() {
        let content: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut app = app_with(&content, 40, 10);
        let cursor_before = app.active_doc().cursors.primary().position;
        click(&mut app, MouseKind::ScrollDown, 0, 0);
        assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(3));
        assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
    }

    #[test]
    fn drag_after_a_plain_click_extends_the_selection() {
        let content: String = (0..5).map(|i| format!("line {i}\n")).collect();
        let mut app = app_with(&content, 40, 10);
        click(&mut app, MouseKind::Down(MouseButton::Left), 0, 0);
        click(&mut app, MouseKind::Drag(MouseButton::Left), 4, 2);
        let c = app.active_doc().cursors.primary();
        assert!(c.has_selection());
        assert_eq!(c.selection_start(), 0);
    }

    #[test]
    fn alt_click_adds_a_cursor_without_disturbing_the_first() {
        let mut app = app_with("hello world\n", 40, 10);
        click(&mut app, MouseKind::Down(MouseButton::Left), 0, 0);
        click_modified(
            &mut app,
            MouseKind::Down(MouseButton::Left),
            6,
            0,
            Modifiers {
                alt: true,
                ..Modifiers::default()
            },
        );
        assert!(app.active_doc().cursors.is_multi());
    }

    #[test]
    fn shift_click_extends_the_selection() {
        let mut app = app_with("hello world\n", 40, 10);
        click(&mut app, MouseKind::Down(MouseButton::Left), 0, 0);
        click_modified(
            &mut app,
            MouseKind::Down(MouseButton::Left),
            5,
            0,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        let c = app.active_doc().cursors.primary();
        assert_eq!(c.selection_range(), (0, 5));
    }

    #[test]
    fn click_outside_the_editor_rect_is_ignored() {
        let mut app = app_with("hello\n", 40, 10);
        let cursor_before = app.active_doc().cursors.primary().position;
        // Row far below the editor's visible area.
        click(&mut app, MouseKind::Down(MouseButton::Left), 0, 200);
        assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
    }

    /// WP3.S5: a click on a synthesised table border row must be a
    /// complete no-op — never move the caret to some nearby
    /// offset. The table sits at the very top of the document, so editor
    /// row 0 is its synthesised `┌┬┐` top border (`DisplaySnapshot::
    /// expand_tables`), with no wrap row of its own to click into. The
    /// cursor is placed on the trailing "tail" paragraph BEFORE the
    /// initial `sync_view` (not via a click, per `app_with`'s own docs:
    /// `doc.view` is cached once per batch, so a click's hit-test always
    /// sees the reveal state as of that initial sync) — otherwise the
    /// default cursor at buffer offset 0 sits ON the table's own line,
    /// which keeps it `Revealed` (raw text, no borders at all) and the
    /// premise of this test never holds.
    #[test]
    fn click_on_a_synthetic_table_border_row_does_not_move_the_cursor() {
        let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n\ntail\n";
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        app.clock = Arc::new(ManualClock::new());
        app.frame_width = 40;
        app.frame_height = 21; // + footer row
        let cursor_offset = content.find("tail").expect("fixture has a tail paragraph");
        let id = app.active;
        app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset);
        app.sync_view();
        let cursor_before = app.active_doc().cursors.primary().position;

        click(&mut app, MouseKind::Down(MouseButton::Left), 2, 0);

        assert_eq!(
            app.active_doc().cursors.primary().position,
            cursor_before,
            "a click on the synthesised top border must not move the caret"
        );
        assert!(!app.active_doc().cursors.primary().has_selection());
    }

    /// Finding 6: the shared click-count -> cursor shape, document-agnostic.
    #[test]
    fn place_click_cursor_and_extend_drag_cursor_are_document_agnostic() {
        let mut doc = crate::document::Document::new(Buffer::new("hello world\n"));
        assert!(place_click_cursor(&mut doc, 6, 6, 1));
        assert_eq!(doc.cursors.primary().position, 6);
        assert!(!place_click_cursor(&mut doc, 6, 6, 2));
        assert_eq!(doc.cursors.primary().selection_range(), (6, 11));
        assert!(!place_click_cursor(&mut doc, 6, 6, 3));
        assert_eq!(doc.cursors.primary().selection_range(), (0, 12));
        extend_drag_cursor(&mut doc, 0, 5, 5);
        let c = doc.cursors.primary();
        assert_eq!((c.anchor, c.position), (0, 5));
    }
}
