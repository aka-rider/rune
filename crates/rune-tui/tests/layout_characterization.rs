//! Characterization tests pinning today's pre-drag left-column geometry.
//!
//! A later refactor replaces the fixed `Percentage(50)` Explorer/Tabs split
//! inside the left column with a user-draggable one, whose fallback (used
//! until the user ever drags the divider) must reproduce exactly what
//! ratatui's constraint solver produces today. These tests exist to pin
//! those numbers down first, so that later change can be checked against a
//! fixed contract instead of against a moving target: if the fallback
//! expression ever disagrees with what is recorded here, the tests below
//! fail and say so, rather than the drift going unnoticed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::layout::Rect;
use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::layout;
use rune_vfs::Mem;

fn app_for() -> App {
    App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
}

#[test]
fn left_column_inner_split_at_120x34() {
    let mut app = app_for();
    app.splits.left.show();
    let geo = layout::geometry(Rect::new(0, 0, 120, 34), &app);

    let left_block = geo.left_block.expect("left pane wide enough to show");
    let divider = geo.tabs_divider.expect("tall enough for a divider row");

    assert_eq!(left_block, Rect::new(0, 0, 22, 33));
    assert_eq!(geo.explorer_inner, Rect::new(1, 1, 20, 16));
    assert_eq!(divider, Rect::new(1, 17, 20, 1));
    assert_eq!(geo.tabs_inner, Rect::new(1, 18, 20, 14));
}

/// Both sizes here matter, not just one: tracing the frame -> `main` ->
/// `left_area.inner(Margin::new(1, 1))` chain, `inner.height` is
/// `frame_height - 3`. A 34-row frame (used above) and a 24-row frame both
/// give an ODD inner height (31 and 21) — pinning only those would leave the
/// solver's rounding behaviour on EVEN inner heights completely
/// unconstrained, and a later fallback expression could be fitted on a
/// one-sided sample that happens to agree on odd heights and silently
/// diverges on even ones. A 25-row frame gives an EVEN inner height (22), so
/// pinning both parities here is what makes the contract actually load-
/// bearing.
#[test]
fn left_column_inner_split_at_80x24() {
    let mut app = app_for();
    app.splits.left.show();
    let geo = layout::geometry(Rect::new(0, 0, 80, 24), &app);

    let left_block = geo.left_block.expect("left pane wide enough to show");
    let divider = geo.tabs_divider.expect("tall enough for a divider row");

    assert_eq!(left_block, Rect::new(0, 0, 22, 23));
    assert_eq!(geo.explorer_inner, Rect::new(1, 1, 20, 11));
    assert_eq!(divider, Rect::new(1, 12, 20, 1));
    assert_eq!(geo.tabs_inner, Rect::new(1, 13, 20, 9));
}

#[test]
fn left_column_inner_split_at_80x25() {
    let mut app = app_for();
    app.splits.left.show();
    let geo = layout::geometry(Rect::new(0, 0, 80, 25), &app);

    let left_block = geo.left_block.expect("left pane wide enough to show");
    let divider = geo.tabs_divider.expect("tall enough for a divider row");

    assert_eq!(left_block, Rect::new(0, 0, 22, 24));
    assert_eq!(geo.explorer_inner, Rect::new(1, 1, 20, 11));
    assert_eq!(divider, Rect::new(1, 12, 20, 1));
    assert_eq!(geo.tabs_inner, Rect::new(1, 13, 20, 10));
}
