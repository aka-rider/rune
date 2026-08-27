#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use super::*;
use crate::pointer::ManualClock;
use crate::runtime::Msg;

fn app_with_palette_open(content: &str, width: u16, height: u16) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.clock = Arc::new(ManualClock::new());
    app.frame_width = width;
    app.frame_height = height;
    app.sync_view();
    let mut effects = crate::runtime::Effects::default();
    crate::palette::open(&mut app, &mut effects);
    app
}

fn palette_rect(app: &App) -> ratatui::layout::Rect {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    crate::layout::geometry(area, app)
        .palette
        .expect("palette must be open")
}

fn mouse_event(app: &mut App, kind: MouseKind, column: u16, row: u16) {
    let mut effects = crate::runtime::Effects::default();
    crate::app::update(
        app,
        Msg::Mouse(MouseInput {
            kind,
            column,
            row,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
}

/// Item 2's own acceptance test: a wheel tick landing inside the open
/// palette must move ITS OWN selection, not scroll whatever pane happens to
/// sit underneath the floating overlay.
#[test]
fn wheel_inside_the_open_palette_moves_its_selection_and_leaves_the_editor_untouched() {
    let mut app = app_with_palette_open("hello", 120, 34);
    let cursor_before = app.palette().expect("open").nav.cursor;
    let scroll_before = app.active_doc().viewport.scroll_row;
    let rect = palette_rect(&app);

    mouse_event(&mut app, MouseKind::ScrollDown, rect.x + 2, rect.y + 2);

    assert!(
        app.palette().is_some(),
        "the wheel must not close the palette"
    );
    assert_eq!(
        app.palette().expect("still open").nav.cursor,
        cursor_before + WHEEL_ROWS as usize,
        "the wheel must move the palette's own selection"
    );
    assert_eq!(
        app.active_doc().viewport.scroll_row,
        scroll_before,
        "the editor's own viewport must not move"
    );
}

/// A wheel tick OUTSIDE the palette's rect (but with the palette still
/// open) is unchanged by this fix — it keeps falling through to whatever
/// pane sits there, exactly as before.
#[test]
fn wheel_outside_the_open_palette_still_falls_through() {
    let tall_content = "line\n".repeat(200);
    let mut app = app_with_palette_open(&tall_content, 120, 34);
    let scroll_before = app.active_doc().viewport.scroll_row;
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let editor = crate::layout::geometry(area, &app).editor;

    mouse_event(&mut app, MouseKind::ScrollDown, editor.x, editor.y);

    assert!(app.palette().is_some(), "the palette must stay open");
    assert_ne!(
        app.active_doc().viewport.scroll_row,
        scroll_before,
        "a tick outside the palette's own rect must still scroll the editor"
    );
}

/// Item 2's own acceptance test: a left-click on a palette row must run it —
/// the same landing a keyboard Down-then-Enter (or, here, a filtered query
/// plus Enter) produces.
#[test]
fn clicking_a_palette_row_behaves_like_enter_on_it() {
    let mut app = app_with_palette_open("hello", 120, 34);
    if let Some(state) = app.palette_mut() {
        state.field.set_text("toggle explorer");
    }
    crate::palette::recompute(&mut app);
    let row_index = app
        .palette()
        .expect("open")
        .rows
        .iter()
        .position(|row| crate::registry::spec(row.id).is_some_and(|s| s.name == "toggle explorer"))
        .expect("the filtered query must match its own row");
    let rect = palette_rect(&app);

    mouse_event(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        rect.x + 2,
        rect.y + 2 + row_index as u16,
    );

    assert!(app.palette().is_none(), "running a row closes the palette");
    assert!(app.splits.left.is_shown(), "the row's own command must run");
    assert_eq!(app.focus(), Pane::Explorer);
}

/// A click landing inside the palette but NOT on any row (the query bar
/// itself) must neither close the palette nor run anything.
#[test]
fn clicking_the_palette_query_bar_does_nothing() {
    let mut app = app_with_palette_open("hello", 120, 34);
    let cursor_before = app.palette().expect("open").nav.cursor;
    let rect = palette_rect(&app);

    mouse_event(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        rect.x + 2,
        rect.y + 1,
    );

    assert!(
        app.palette().is_some(),
        "the query bar is still inside the rect"
    );
    assert_eq!(app.palette().expect("still open").nav.cursor, cursor_before);
}

/// A click OUTSIDE the palette's own rect still cancels it, exactly as
/// before this fix.
#[test]
fn clicking_outside_the_palette_rect_still_cancels_it() {
    let mut app = app_with_palette_open("hello", 120, 34);
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let editor = crate::layout::geometry(area, &app).editor;

    mouse_event(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        editor.x,
        editor.y,
    );

    assert!(app.palette().is_none(), "an outside click must cancel");
}

/// A drag started inside the open palette moves its selection to whatever
/// row the pointer is over, without running it. Starts the press on the
/// query bar itself (a row that runs nothing on press), so the gesture is
/// still latched by the time it reaches a real row.
#[test]
fn dragging_inside_the_open_palette_moves_the_selection_without_running_it() {
    let mut app = app_with_palette_open("hello", 120, 34);
    let rect = palette_rect(&app);

    mouse_event(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        rect.x + 2,
        rect.y + 1,
    );
    assert!(
        app.palette().is_some(),
        "the query bar press must not close it"
    );

    mouse_event(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        rect.x + 2,
        rect.y + 3,
    );

    assert!(app.palette().is_some(), "a drag must never run a row");
    assert_eq!(app.palette().expect("still open").nav.cursor, 1);
}
