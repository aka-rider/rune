//! Pure unit tests on `layout::geometry` (plan WP3.S10) — no `Frame`, no
//! `TestBackend`; just `Rect` arithmetic against a bare `App`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::layout::Rect;
use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::layout::{self, MIN_CENTER_W, MIN_LEFT_PANE_W};
use rune_vfs::Mem;

fn app_for() -> App {
    App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
}

#[test]
fn hidden_left_pane_at_120x34_gives_the_center_the_whole_main_area() {
    let app = app_for();
    let geo = layout::geometry(Rect::new(0, 0, 120, 34), &app);

    assert_eq!(geo.footer, Rect::new(0, 33, 120, 1));
    assert_eq!(geo.center, Rect::new(0, 0, 120, 33));
    assert!(geo.left_block.is_none());
    assert!(geo.tabs_divider.is_none());

    // Plan WP4.S1: the center pane is bordered (120x33 is well over the
    // 3x3 floor), so `content` is `center.inner(Margin::new(1, 1))` —
    // title and editor both start one cell in from `center`'s own origin.
    assert!(geo.center_bordered);
    assert_eq!(geo.title, Some(Rect::new(1, 1, 118, 1)));
    assert_eq!(geo.editor, Rect::new(1, 2, 118, 30));
}

#[test]
fn visible_left_pane_at_120x34_gives_it_the_default_width() {
    let mut app = app_for();
    app.left_visible = true;
    let geo = layout::geometry(Rect::new(0, 0, 120, 34), &app);

    let left_block = geo.left_block.expect("left pane wide enough to show");
    assert_eq!(left_block.width, 22);
    // ONE block for the whole left column: it spans the entire main area
    // (everything above the footer), not just its upper half.
    assert_eq!(left_block, Rect::new(0, 0, 22, 33));
    // The center pane starts at x=22 (past the 22-wide left column) but is
    // itself bordered too, so the editor's content starts one more cell in.
    assert_eq!(geo.center.x, 22);
    assert_eq!(geo.editor.x, 23);
}

/// The `Open` divider sits INSIDE the single border (one cell in on each
/// side) and immediately between the Explorer rows and the tab rows, with
/// no gap and no overlap on either side.
#[test]
fn the_tabs_divider_sits_between_the_two_inner_sections() {
    let mut app = app_for();
    app.left_visible = true;
    let geo = layout::geometry(Rect::new(0, 0, 120, 34), &app);

    let left_block = geo.left_block.expect("left pane wide enough to show");
    let divider = geo.tabs_divider.expect("tall enough for a divider row");

    assert_eq!(divider.height, 1);
    assert_eq!(divider.x, left_block.x + 1);
    assert_eq!(divider.width, left_block.width - 2);

    assert_eq!(geo.explorer_inner.x, divider.x);
    assert_eq!(geo.explorer_inner.width, divider.width);
    assert_eq!(geo.tabs_inner.x, divider.x);
    assert_eq!(geo.tabs_inner.width, divider.width);

    assert_eq!(
        geo.explorer_inner.y + geo.explorer_inner.height,
        divider.y,
        "the divider must start right where the Explorer rows end"
    );
    assert_eq!(
        divider.y + 1,
        geo.tabs_inner.y,
        "the tab rows must start right after the divider"
    );
    assert_eq!(
        geo.tabs_inner.y + geo.tabs_inner.height,
        left_block.y + left_block.height - 1,
        "the tab rows must stop just above the block's bottom border"
    );
}

/// At a height that leaves the block's inner rect a single row, there is no
/// room to spend one on the divider: the Explorer takes it all and the Tabs
/// section is absent for that frame.
#[test]
fn a_one_row_inner_rect_has_no_divider() {
    let mut app = app_for();
    app.left_visible = true;

    // 3 main-area rows + 1 footer row: the block's border eats two of the
    // three, leaving exactly one inner row.
    let geo = layout::geometry(Rect::new(0, 0, 120, 4), &app);

    assert!(geo.left_block.is_some());
    assert_eq!(geo.explorer_inner.height, 1);
    assert!(geo.tabs_divider.is_none());
    assert_eq!(geo.tabs_inner.height, 0);
}

#[test]
fn zero_and_one_by_one_areas_never_panic_and_stay_within_bounds() {
    let app = app_for();

    for area in [Rect::new(0, 0, 0, 0), Rect::new(0, 0, 1, 1)] {
        let geo = layout::geometry(area, &app);
        for rect in [geo.footer, geo.center, geo.editor] {
            assert!(rect.width <= area.width, "{rect:?} vs {area:?}");
            assert!(rect.height <= area.height, "{rect:?} vs {area:?}");
        }
        if let Some(r) = geo.left_block {
            assert!(r.width <= area.width && r.height <= area.height);
        }
        if let Some(r) = geo.tabs_divider {
            assert!(r.width <= area.width && r.height <= area.height);
        }
        if let Some(r) = geo.title {
            assert!(r.width <= area.width && r.height <= area.height);
        }
    }
}

#[test]
fn too_narrow_for_both_minimums_drops_the_left_pane() {
    let mut app = app_for();
    app.left_visible = true;
    let width = MIN_LEFT_PANE_W + MIN_CENTER_W - 10; // 30, well under the 40 floor
    let geo = layout::geometry(Rect::new(0, 0, width, 34), &app);

    assert!(geo.left_block.is_none());
    assert!(geo.tabs_divider.is_none());
    assert_eq!(geo.center.width, width);
}
