use super::*;
use rune_core::buffer::Buffer;
use rune_vfs::Mem;
use std::sync::Arc;

/// The bar's one row appears between `title` and `editor`
/// only while `App::search` is open, and reserving it shrinks `editor`
/// by exactly one row rather than displacing `title`.
#[test]
fn an_open_search_bar_reserves_one_row_between_title_and_editor() {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    let area = Rect::new(0, 0, 120, 34);

    let closed = geometry(area, &app);
    assert!(closed.search_bar.is_none());

    crate::search::open(&mut app);
    let open = geometry(area, &app);
    let bar = open.search_bar.expect("bar row while App::search is open");
    let title = open.title.expect("title row at this frame size");

    assert_eq!(bar.y, title.y + 1);
    assert_eq!(bar.height, 1);
    assert_eq!(bar.x, title.x);
    assert_eq!(bar.width, title.width);
    assert_eq!(open.editor.y, closed.editor.y + 1);
    assert_eq!(open.editor.height, closed.editor.height - 1);
}

/// A content area only one row tall keeps that row for the title
/// instead — the bar simply has no room and `saturating_sub` keeps
/// `editor` from underflowing.
#[test]
fn a_one_row_content_area_gives_the_bar_no_room_and_never_panics() {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    crate::search::open(&mut app);
    // Tall enough for the footer alone plus one content row.
    let geo = geometry(Rect::new(0, 0, 40, 2), &app);
    assert!(geo.search_bar.is_none());
}

/// `explorer_budget` is a public function precisely because `geometry`
/// and the focus-tabs handler's `ensure_trail` call must both use the exact
/// same quantity — pin that it agrees with the inner rect `geometry`
/// itself carves the vertical split from, rather than trusting the two
/// expressions to stay in sync by inspection.
#[test]
fn explorer_budget_matches_the_inner_rect_geometry_actually_splits() {
    for left_area in [
        Rect::new(0, 0, 22, 33),
        Rect::new(0, 0, 22, 23),
        Rect::new(0, 0, 22, 24),
    ] {
        let inner = left_area.inner(Margin::new(1, 1));
        assert_eq!(explorer_budget(left_area), inner.height.saturating_sub(1));
    }
}

fn app_with_left_shown() -> App {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    app.splits.left.show();
    app
}

/// A frame too NARROW to fit the left column alongside the center pane
/// must still show the column the user asked for — flipping to a
/// full-width `LayoutMode::ExplorerOnly` rather than silently dropping
/// it to `EditorOnly` the way the pre-flip resolver used to.
#[test]
fn a_too_narrow_frame_flips_to_explorer_only_instead_of_dropping_the_column() {
    let app = app_with_left_shown();
    let area = Rect::new(0, 0, MIN_LEFT_PANE_W + MIN_CENTER_W - 1, 30);
    let geo = geometry(area, &app);
    assert!(geo.left_block.is_some());
    assert_eq!(
        geo.left_block,
        Some(Rect::new(area.x, area.y, area.width, area.height - 1))
    );
    assert_eq!(resolve_mode(area, &app), LayoutMode::ExplorerOnly);
}

/// The converse: the same too-narrow frame with the column HIDDEN
/// resolves to a full-width `EditorOnly` — the flip is keyed on whether
/// the user asked for the column, never applied unconditionally.
#[test]
fn a_too_narrow_frame_with_the_column_hidden_stays_editor_only() {
    let app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    assert!(!app.splits.left.is_shown());
    let area = Rect::new(0, 0, MIN_LEFT_PANE_W + MIN_CENTER_W - 1, 30);
    assert!(geometry(area, &app).left_block.is_none());
    assert_eq!(resolve_mode(area, &app), LayoutMode::EditorOnly);
}

/// A frame narrow AND short enough that the column can't show anything
/// even at full width still falls back to `EditorOnly` — the flip only
/// ever trades a dropped column for a full-width one when there is
/// something to actually paint there.
#[test]
fn a_too_narrow_and_too_short_frame_still_falls_back_to_editor_only() {
    let app = app_with_left_shown();
    let area = Rect::new(0, 0, MIN_LEFT_PANE_W + MIN_CENTER_W - 1, 3);
    assert!(geometry(area, &app).left_block.is_none());
    assert_eq!(resolve_mode(area, &app), LayoutMode::EditorOnly);
}

/// The height-driven counterpart: a column with enough WIDTH to show
/// SIDE BY SIDE with the center pane, but too few ROWS for either the
/// Explorer or the tab rows to fit (`resolve`'s `(None, None)` arm),
/// resolves to `EditorOnly` — this frame is wide enough that the
/// narrow-frame flip above never applies; it fails on height alone.
#[test]
fn a_too_short_frame_resolves_to_editor_only_not_a_silently_dropped_column() {
    let app = app_with_left_shown();
    let area = Rect::new(0, 0, 40, 3);
    assert!(geometry(area, &app).left_block.is_none());
    assert_eq!(resolve_mode(area, &app), LayoutMode::EditorOnly);
}

/// The ordinary case: a roomy frame resolves to `Split` with both
/// sections painted, matching `geometry`'s own rects.
#[test]
fn a_roomy_frame_resolves_to_split_with_both_sections_shown() {
    let app = app_with_left_shown();
    let area = Rect::new(0, 0, 100, 30);
    let geo = geometry(area, &app);
    assert!(geo.left_block.is_some());
    assert!(geo.explorer_inner.height > 0);
    assert!(geo.tabs_inner.height > 0);
    assert_eq!(
        resolve_mode(area, &app),
        LayoutMode::Split {
            explorer: true,
            tabs: true
        }
    );
}

/// The load-bearing acceptance case: opening the finder
/// on a fresh app whose left column was NEVER shown still forces the
/// column visible, at `min(max(FILESEARCH_MIN_W, size_hint), frame -
/// MIN_CENTER_W)` — geometry decides visibility here, `app.splits` is
/// never written.
#[test]
fn filesearch_forces_the_column_visible_at_its_own_minimum_width() {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    assert!(!app.splits.left.is_shown(), "test setup: column hidden");
    let area = Rect::new(0, 0, 120, 34);
    let mut effects = crate::runtime::Effects::default();

    crate::filesearch::open(&mut app, &mut effects);

    let geo = geometry(area, &app);
    let left_area = geo.left_block.expect("column forced visible");
    assert_eq!(left_area.width, FILESEARCH_MIN_W);
    assert_eq!(geo.center.width, area.width - FILESEARCH_MIN_W);
    assert!(
        !app.splits.left.is_shown(),
        "the override never writes app.splits"
    );
}

/// A frame between 40 and 72 columns wide (fits the ordinary floor but
/// not the finder's own 48-cell minimum) still shows the column, at
/// `frame - MIN_CENTER_W` rather than the full 48.
#[test]
fn filesearch_narrows_below_its_own_minimum_when_the_frame_is_tight() {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    let area = Rect::new(0, 0, 60, 34);
    let mut effects = crate::runtime::Effects::default();

    crate::filesearch::open(&mut app, &mut effects);

    let geo = geometry(area, &app);
    let left_area = geo.left_block.expect("column still forced visible");
    assert_eq!(left_area.width, area.width - MIN_CENTER_W);
    assert!(left_area.width < FILESEARCH_MIN_W);
    assert!(left_area.width >= MIN_LEFT_PANE_W);
}

/// Below `split_fits` (< 40 columns) with the left column never shown,
/// the finder must still be painted — falling through to the same
/// full-width `ExplorerOnly` branch an already-shown column takes,
/// rather than the ordinary `Split::allot` path, which returns `None`
/// for a hidden column and would leave the finder invisible while it
/// keeps consuming every keystroke (silent input swallowing).
#[test]
fn filesearch_paints_explorer_only_below_split_fits_on_a_never_shown_column() {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    assert!(!app.splits.left.is_shown(), "test setup: column hidden");
    let area = Rect::new(0, 0, 30, 34);
    let mut effects = crate::runtime::Effects::default();

    crate::filesearch::open(&mut app, &mut effects);

    let geo = geometry(area, &app);
    assert!(geo.left_block.is_some(), "the finder must still be painted");
    assert_eq!(resolve_mode(area, &app), LayoutMode::ExplorerOnly);
}

fn within(inner: Rect, outer: Rect) -> bool {
    if inner.width == 0 || inner.height == 0 {
        return true;
    }
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

fn assert_geometry_within(geo: &Geometry, frame: Rect) {
    let mut rects: Vec<(&str, Rect)> = vec![
        ("footer", geo.footer),
        ("explorer_inner", geo.explorer_inner),
        ("tabs_inner", geo.tabs_inner),
        ("center", geo.center),
        ("editor", geo.editor),
        ("main", geo.main),
    ];
    if let Some(r) = geo.messages {
        rects.push(("messages", r));
    }
    if let Some(r) = geo.left_block {
        rects.push(("left_block", r));
    }
    if let Some(r) = geo.tabs_divider {
        rects.push(("tabs_divider", r));
    }
    if let Some(r) = geo.title {
        rects.push(("title", r));
    }
    if let Some(r) = geo.search_bar {
        rects.push(("search_bar", r));
    }
    if let Some(r) = geo.left_splitter {
        rects.push(("left_splitter", r));
    }
    for (name, rect) in rects {
        assert!(
            within(rect, frame),
            "{name} {rect:?} does not lie inside frame {frame:?}"
        );
    }
}

#[test]
fn every_region_lies_inside_the_frame_across_degenerate_sizes() {
    let mut shapes: Vec<(u16, u16)> = Vec::new();
    for w in 1..=6u16 {
        for h in 1..=6u16 {
            shapes.push((w, h));
        }
    }
    shapes.extend([(1, 60), (60, 1), (200, 1), (1, 200), (1, 1), (2, 1)]);

    for (w, h) in shapes {
        let area = Rect::new(0, 0, w, h);
        for left_shown in [false, true] {
            let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
            if left_shown {
                app.splits.left.show();
            }
            let geo = geometry(area, &app);
            assert_geometry_within(&geo, area);
        }
    }
}

/// Closing the finder restores IDENTICAL geometry to a plain
/// never-opened app at the same frame — nothing was ever written to
/// `app.splits`, so there is nothing to restore.
#[test]
fn closing_filesearch_restores_the_pre_open_geometry() {
    let baseline = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    let area = Rect::new(0, 0, 120, 34);
    let before = geometry(area, &baseline);

    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    let mut effects = crate::runtime::Effects::default();
    crate::filesearch::open(&mut app, &mut effects);
    assert!(
        geometry(area, &app).left_block.is_some(),
        "test setup: finder open"
    );
    crate::filesearch::cancel(&mut app, &mut effects);

    let after = geometry(area, &app);
    assert_eq!(after.left_block, before.left_block);
    assert_eq!(after.center, before.center);
}
