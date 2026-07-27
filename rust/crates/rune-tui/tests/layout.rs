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
    assert!(geo.explorer_block.is_none());
    assert!(geo.tabs_block.is_none());

    // No border yet at this WP (plan WP3.S1): title row 0, breadcrumb row
    // 1, editor from row 2 — a direct port of the pre-WP3 `center_chrome_
    // rows`. WP4 adds the border and removes the reserved breadcrumb row.
    assert!(!geo.center_bordered);
    assert_eq!(geo.title, Some(Rect::new(0, 0, 120, 1)));
    assert_eq!(geo.breadcrumb, Some(Rect::new(0, 1, 120, 1)));
    assert_eq!(geo.editor.height, 31);
}

#[test]
fn visible_left_pane_at_120x34_gives_it_the_default_width() {
    let mut app = app_for();
    app.left_visible = true;
    let geo = layout::geometry(Rect::new(0, 0, 120, 34), &app);

    let explorer_block = geo.explorer_block.expect("left pane wide enough to show");
    assert_eq!(explorer_block.width, 22);
    // The center pane starts at x=22 (past the 22-wide left column); no
    // border yet at this WP, so the editor's own x matches it exactly.
    assert_eq!(geo.center.x, 22);
    assert_eq!(geo.editor.x, 22);
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
        if let Some(r) = geo.explorer_block {
            assert!(r.width <= area.width && r.height <= area.height);
        }
        if let Some(r) = geo.tabs_block {
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

    assert!(geo.explorer_block.is_none());
    assert!(geo.tabs_block.is_none());
    assert_eq!(geo.center.width, width);
}
