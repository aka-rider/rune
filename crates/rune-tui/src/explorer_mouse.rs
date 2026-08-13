use ratatui::layout::Rect;

use crate::app::App;
use crate::commands::mouse::WHEEL_ROWS;
use crate::explorer;
use crate::explorer_keys;
use crate::pane::Pane;
use crate::pointer::{MouseButton, MouseInput, MouseKind};
use crate::runtime::Effects;

fn pane_rect(app: &App) -> Rect {
    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    crate::layout::geometry(area, app).explorer_inner
}

pub(crate) fn mouse(app: &mut App, input: MouseInput, effects: &mut Effects) {
    match input.kind {
        MouseKind::ScrollUp => scroll(app, -WHEEL_ROWS),
        MouseKind::ScrollDown => scroll(app, WHEEL_ROWS),
        MouseKind::Down(MouseButton::Left) => mouse_down(app, input, effects),
        _ => {}
    }
}

fn scroll(app: &mut App, delta: isize) {
    let rect = pane_rect(app);
    let len = app.explorer.entries.len();
    app.explorer
        .nav
        .scroll_by(delta, len, explorer::entry_rows(rect));
}

fn mouse_down(app: &mut App, input: MouseInput, effects: &mut Effects) {
    let rect = pane_rect(app);
    let index = input
        .row
        .checked_sub(rect.y)
        .and_then(|row| explorer::entry_at(app, rect, row));

    let Some(index) = index else {
        app.pointer.end_click_run();
        app.set_focus_pane(Pane::Explorer, effects);
        return;
    };

    let now = app.clock.now();
    let count = app
        .pointer
        .register_row_click(now, input.column, input.row, index);

    app.set_focus_pane(Pane::Explorer, effects);
    if app.focus() != Pane::Explorer {
        return;
    }
    explorer_keys::select_index(app, index, effects);
    if count >= 2 {
        explorer_keys::open_selected(app, effects);
    }
}
