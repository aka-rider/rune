//! Mouse-drag pane resizing: grabbing the left column's border band or
//! the `Open` divider row and dragging moves the corresponding `Split`,
//! same shape as the plain click/drag test helper used for text selection
//! elsewhere, extended to emit `Down` -> `Drag` -> `Up` runs at absolute
//! frame coordinates (the splitter bands live outside the editor rect, so a
//! gesture here can't be expressed relative to it the way a text-selection
//! click can).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ratatui::layout::Rect;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::layout::{self, DEFAULT_LEFT_PANE_W, Geometry, MIN_CENTER_W, MIN_LEFT_PANE_W};
use rune_tui::pane::Pane;
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

/// A fresh app with the left column shown at whatever width the frame
/// defaults to, sized to `width`x`height`.
fn app_for(width: u16, height: u16) -> App {
    let mut app = App::new(Buffer::new("hello\n"), None, Arc::new(Mem::new()), None);
    app.frame_width = width;
    app.frame_height = height;
    app.splits.left.show();
    app.sync_view();
    app
}

/// The same geometry `commands::mouse`/`commands::splitter` themselves
/// read from — never re-derived independently.
fn geo(app: &App) -> Geometry {
    layout::geometry(Rect::new(0, 0, app.frame_width, app.frame_height), app)
}

/// Sends one raw mouse event through the real `update`, resyncing
/// afterward (what the runtime does once per message batch) so a later
/// assertion sees the settled geometry.
fn send(app: &mut App, kind: MouseKind, col: u16, row: u16) {
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Mouse(MouseInput {
            kind,
            column: col,
            row,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
    app.sync_view();
}

/// `^b` — this tree's binding for "expose and focus the Explorer" (the
/// plan calls it `FocusExplorer`; WP5 lands the rename separately).
fn expose_explorer(app: &mut App) {
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('b'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );
    app.sync_view();
}

#[test]
fn dragging_the_left_splitter_right_widens_the_column() {
    let mut app = app_for(100, 30);
    let splitter = geo(&app).left_splitter.expect("column is shown");
    let before_w = geo(&app).left_block.expect("column is shown").width;

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        splitter.x,
        splitter.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        splitter.x + 5,
        splitter.y,
    );
    send(
        &mut app,
        MouseKind::Up(MouseButton::Left),
        splitter.x + 5,
        splitter.y,
    );

    let after_w = geo(&app).left_block.expect("still shown").width;
    assert_eq!(after_w, before_w + 5);
}

#[test]
fn dragging_the_left_splitter_past_the_floor_collapses_the_column() {
    let mut app = app_for(100, 30);
    let splitter = geo(&app).left_splitter.expect("column is shown");

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        splitter.x,
        splitter.y,
    );
    send(&mut app, MouseKind::Drag(MouseButton::Left), 0, splitter.y);
    send(&mut app, MouseKind::Up(MouseButton::Left), 0, splitter.y);

    assert!(geo(&app).left_block.is_none());
}

#[test]
fn re_exposing_after_a_collapse_restores_the_dragged_width_not_the_default() {
    let mut app = app_for(100, 30);
    let splitter = geo(&app).left_splitter.expect("column is shown");
    let before_w = geo(&app).left_block.expect("shown").width;
    let dragged_w = before_w + 8;

    // Widen to a known, non-default width first.
    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        splitter.x,
        splitter.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        splitter.x + 8,
        splitter.y,
    );
    send(
        &mut app,
        MouseKind::Up(MouseButton::Left),
        splitter.x + 8,
        splitter.y,
    );
    assert_eq!(geo(&app).left_block.expect("shown").width, dragged_w);

    // Collapse it by dragging the (now wider) splitter past the floor.
    let splitter2 = geo(&app)
        .left_splitter
        .expect("still shown before collapse");
    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        splitter2.x,
        splitter2.y,
    );
    send(&mut app, MouseKind::Drag(MouseButton::Left), 0, splitter2.y);
    send(&mut app, MouseKind::Up(MouseButton::Left), 0, splitter2.y);
    assert!(geo(&app).left_block.is_none());

    expose_explorer(&mut app);

    let restored_w = geo(&app).left_block.expect("re-exposed").width;
    assert_eq!(restored_w, dragged_w);
    assert_ne!(restored_w, DEFAULT_LEFT_PANE_W);
}

#[test]
fn dragging_the_tabs_divider_down_grows_explorer_and_shrinks_tabs_by_the_same_amount() {
    let mut app = app_for(100, 30);
    let g0 = geo(&app);
    let divider = g0.tabs_divider.expect("divider is shown");
    let explorer_h0 = g0.explorer_inner.height;
    let tabs_h0 = g0.tabs_inner.height;

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        divider.x,
        divider.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        divider.x,
        divider.y + 3,
    );
    send(
        &mut app,
        MouseKind::Up(MouseButton::Left),
        divider.x,
        divider.y + 3,
    );

    let g1 = geo(&app);
    assert_eq!(g1.explorer_inner.height, explorer_h0 + 3);
    assert_eq!(g1.tabs_inner.height, tabs_h0.saturating_sub(3));
}

#[test]
fn dragging_the_divider_to_the_top_collapses_the_explorer_but_keeps_the_divider() {
    let mut app = app_for(100, 30);
    let g0 = geo(&app);
    let divider = g0.tabs_divider.expect("divider is shown");
    let left_block = g0.left_block.expect("column is shown");

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        divider.x,
        divider.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        divider.x,
        left_block.y,
    );
    send(
        &mut app,
        MouseKind::Up(MouseButton::Left),
        divider.x,
        left_block.y,
    );

    let g1 = geo(&app);
    assert_eq!(g1.explorer_inner.height, 0);
    assert!(g1.tabs_divider.is_some());
}

/// Dragging the divider down to the block's bottom border collapses the tab
/// rows and hands the Explorer the whole inner rect. Overshooting is the
/// natural way to perform this gesture — nobody lands the pointer on the one
/// exact row that leaves the trail just under its floor — so the collapse
/// must survive a request larger than the axis, not only one that fits it.
#[test]
fn dragging_the_divider_to_the_bottom_collapses_the_tab_rows() {
    let mut app = app_for(100, 30);
    let g0 = geo(&app);
    let divider = g0.tabs_divider.expect("divider is shown");
    let left_block = g0.left_block.expect("column is shown");
    let bottom = left_block.y.saturating_add(left_block.height);

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        divider.x,
        divider.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        divider.x,
        bottom,
    );
    send(
        &mut app,
        MouseKind::Up(MouseButton::Left),
        divider.x,
        bottom,
    );

    let g1 = geo(&app);
    assert!(
        g1.tabs_divider.is_none(),
        "dragging to the bottom border must collapse the tab rows outright"
    );
    assert_eq!(
        g1.explorer_inner.height,
        left_block.height.saturating_sub(2)
    );
}

#[test]
fn shrinking_and_restoring_the_frame_preserves_the_dragged_width() {
    let mut app = app_for(100, 30);
    let splitter = geo(&app).left_splitter.expect("column is shown");
    let before_w = geo(&app).left_block.expect("shown").width;
    let dragged_w = before_w + 10;

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        splitter.x,
        splitter.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        splitter.x + 10,
        splitter.y,
    );
    send(
        &mut app,
        MouseKind::Up(MouseButton::Left),
        splitter.x + 10,
        splitter.y,
    );
    assert_eq!(geo(&app).left_block.expect("shown").width, dragged_w);

    // Narrow below the point where both floors can fit: the column drops.
    app.frame_width = MIN_LEFT_PANE_W + MIN_CENTER_W - 1;
    app.sync_view();
    assert!(
        geo(&app).left_block.is_none(),
        "column must drop below the combined floor"
    );

    // Restore: the DESIRED size was never written back, so it comes back
    // untouched rather than resetting to the default.
    app.frame_width = 100;
    app.sync_view();
    assert_eq!(geo(&app).left_block.expect("restored").width, dragged_w);
}

/// The vertical counterpart of the width test above. Drag the tabs divider
/// well down, then shrink the frame's HEIGHT past what that drag asked for:
/// the tab rows collapse, because the collapse rule applies to whatever the
/// frame can actually grant, not only to a fresh gesture. Restoring the
/// height brings them back — the dragged size is never written down to fit a
/// smaller frame, so nothing is lost.
#[test]
fn shrinking_the_frame_height_collapses_the_tab_rows_and_restoring_it_brings_them_back() {
    let mut app = app_for(100, 30);
    let g0 = geo(&app);
    let divider = g0.tabs_divider.expect("divider is shown");

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        divider.x,
        divider.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        divider.x,
        divider.y + 6,
    );
    send(
        &mut app,
        MouseKind::Up(MouseButton::Left),
        divider.x,
        divider.y + 6,
    );
    assert!(
        geo(&app).tabs_divider.is_some(),
        "divider must still be shown right after the drag"
    );
    let explorer_after_drag = geo(&app).explorer_inner.height;

    app.frame_height = 15;
    app.sync_view();
    assert!(
        geo(&app).tabs_divider.is_none(),
        "a height that can no longer grant the dragged size collapses the tab rows"
    );

    app.frame_height = 30;
    app.sync_view();
    assert!(
        geo(&app).tabs_divider.is_some(),
        "restoring the height must bring the tab rows back"
    );
    assert_eq!(geo(&app).explorer_inner.height, explorer_after_drag);
}

#[test]
fn collapsing_the_focused_explorer_by_dragging_hands_focus_to_the_editor() {
    let mut app = app_for(100, 30);
    app.focus = Pane::Explorer;
    let g0 = geo(&app);
    let divider = g0.tabs_divider.expect("divider is shown");
    let left_block = g0.left_block.expect("column is shown");

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        divider.x,
        divider.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        divider.x,
        left_block.y,
    );
    send(
        &mut app,
        MouseKind::Up(MouseButton::Left),
        divider.x,
        left_block.y,
    );

    assert_eq!(app.focus, Pane::Editor);
}

#[test]
fn a_lost_button_up_does_not_latch_the_pointer_forever() {
    let content: String = (0..50).map(|i| format!("line {i}\n")).collect();
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.frame_width = 100;
    app.frame_height = 30;
    app.splits.left.show();
    app.sync_view();

    let splitter = geo(&app).left_splitter.expect("column is shown");
    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        splitter.x,
        splitter.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        splitter.x + 5,
        splitter.y,
    );
    // No `Up` follows — e.g. the release happened while the terminal
    // wasn't focused. A fresh, unrelated event must still end the
    // gesture rather than being swallowed forever.
    let editor = geo(&app).editor;
    send(&mut app, MouseKind::ScrollDown, editor.x, editor.y);

    assert_eq!(
        app.active_doc().viewport.scroll_row,
        3,
        "the wheel event must fall through to the editor, not be swallowed by a latched drag"
    );
}

#[test]
fn a_drag_that_leaves_the_frame_entirely_neither_panics_nor_moves_the_caret() {
    let mut app = app_for(100, 30);
    let splitter = geo(&app).left_splitter.expect("column is shown");
    let cursor_before = app.active_doc().cursors.primary().position;

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        splitter.x,
        splitter.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        u16::MAX,
        u16::MAX,
    );
    send(
        &mut app,
        MouseKind::Up(MouseButton::Left),
        u16::MAX,
        u16::MAX,
    );

    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
}
