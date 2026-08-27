use super::*;
use rune_core::buffer::Buffer;
use rune_vfs::Mem;
use std::sync::Arc;

#[test]
fn an_open_search_bar_reserves_one_row_between_title_and_editor() {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    let area = Rect::new(0, 0, 120, 34);

    let closed = geometry(area, &app);
    assert!(closed.search_bar.is_none());

    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
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

#[test]
fn a_one_row_content_area_gives_the_bar_no_room_and_never_panics() {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    // Tall enough for the footer alone plus one content row.
    let geo = geometry(Rect::new(0, 0, 40, 2), &app);
    assert!(geo.search_bar.is_none());
}

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

#[test]
fn a_too_narrow_frame_with_the_column_hidden_stays_editor_only() {
    let app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    assert!(!app.splits.left.is_shown());
    let area = Rect::new(0, 0, MIN_LEFT_PANE_W + MIN_CENTER_W - 1, 30);
    assert!(geometry(area, &app).left_block.is_none());
    assert_eq!(resolve_mode(area, &app), LayoutMode::EditorOnly);
}

#[test]
fn a_too_narrow_and_too_short_frame_still_falls_back_to_editor_only() {
    let app = app_with_left_shown();
    let area = Rect::new(0, 0, MIN_LEFT_PANE_W + MIN_CENTER_W - 1, 3);
    assert!(geometry(area, &app).left_block.is_none());
    assert_eq!(resolve_mode(area, &app), LayoutMode::EditorOnly);
}

#[test]
fn a_too_short_frame_resolves_to_editor_only_not_a_silently_dropped_column() {
    let app = app_with_left_shown();
    let area = Rect::new(0, 0, 40, 3);
    assert!(geometry(area, &app).left_block.is_none());
    assert_eq!(resolve_mode(area, &app), LayoutMode::EditorOnly);
}

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
    if let Some(r) = geo.diff_left {
        rects.push(("diff_left", r));
    }
    if let Some(r) = geo.diff_splitter {
        rects.push(("diff_splitter", r));
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

fn corners(rect: Rect) -> [(u16, u16); 2] {
    [
        (rect.x, rect.y),
        (
            rect.right().saturating_sub(1),
            rect.bottom().saturating_sub(1),
        ),
    ]
}

#[test]
fn pane_at_maps_each_rect_to_its_own_pane() {
    let mut app = app_with_left_shown();
    let mut effects = crate::runtime::Effects::default();
    crate::messages::toggle(&mut app, &mut effects);
    let area = Rect::new(0, 0, 80, 24);
    let geo = geometry(area, &app);

    let messages = geo.messages.expect("messages pane open");
    assert!(geo.explorer_inner.height > 0 && geo.tabs_inner.height > 0);

    for (rect, pane) in [
        (messages, crate::pane::Pane::Messages),
        (geo.explorer_inner, crate::pane::Pane::Explorer),
        (geo.tabs_inner, crate::pane::Pane::Tabs),
        (geo.editor, crate::pane::Pane::Editor),
    ] {
        for (column, row) in corners(rect) {
            assert_eq!(
                geo.pane_at(column, row),
                Some(pane),
                "{rect:?} corner ({column}, {row})"
            );
        }
    }
}

#[test]
fn pane_at_leaves_the_chrome_unowned() {
    let mut app = app_with_left_shown();
    let mut effects = crate::runtime::Effects::default();
    crate::messages::toggle(&mut app, &mut effects);
    let area = Rect::new(0, 0, 80, 24);
    let geo = geometry(area, &app);
    let block = geo.left_block.expect("left column shown");
    let divider = geo.tabs_divider.expect("Open divider shown");

    assert_eq!(geo.pane_at(0, geo.footer.y), None, "the footer row");
    assert_eq!(
        geo.pane_at(block.x, geo.explorer_inner.y),
        None,
        "the block's left border column"
    );
    assert_eq!(geo.pane_at(divider.x, divider.y), None, "the Open divider");
}

#[test]
fn pane_at_never_claims_a_zero_sized_section() {
    let app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    let area = Rect::new(0, 0, 80, 24);
    let geo = geometry(area, &app);
    assert_eq!(geo.explorer_inner, Rect::new(0, 0, 0, 0));
    assert_eq!(geo.tabs_inner, Rect::new(0, 0, 0, 0));
    assert_eq!(geo.pane_at(0, 0), None);
}
