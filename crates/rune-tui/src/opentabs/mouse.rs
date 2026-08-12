use ratatui::layout::Rect;

use crate::app::App;
use crate::commands::mouse::WHEEL_ROWS;
use crate::pane::Pane;
use crate::pointer::{MouseButton, MouseInput, MouseKind};
use crate::runtime::Effects;

fn pane_rect(app: &App) -> Rect {
    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    crate::layout::geometry(area, app).tabs_inner
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
    let len = app.documents.order().len();
    app.tabs.nav.scroll_by(delta, len, super::entry_rows(rect));
}

/// The clicked row resolves BEFORE focus moves: landing focus on this pane
/// discards a live Explorer preview, which removes a tab row — resolving
/// afterwards would let a click on the preview's own row name a tab that no
/// longer exists.
fn mouse_down(app: &mut App, input: MouseInput, effects: &mut Effects) {
    let rect = pane_rect(app);
    let index = input
        .row
        .checked_sub(rect.y)
        .and_then(|row| super::entry_at(app, rect, row));

    let Some(index) = index else {
        app.pointer.end_click_run();
        app.set_focus_pane(Pane::Tabs, effects);
        return;
    };

    let now = app.pointer_clock.now();
    let count = app
        .pointer
        .register_row_click(now, input.column, input.row, index);

    app.set_focus_pane(Pane::Tabs, effects);
    if app.focus() != Pane::Tabs {
        return;
    }
    super::select_index(app, index);
    if count >= 2 {
        super::activate(app, effects);
    }
}
