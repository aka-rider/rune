#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod diff_view_common;

use std::sync::Arc;

use diff_view_common::{
    HEIGHT, TOO_NARROW, WIDE_ENOUGH, app_with_diff, ctrl, geo, key, row_strings, send, sup_shift,
};
use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

#[test]
fn take_theirs_makes_the_region_same_and_undoes_in_one_step() {
    let mut app = app_with_diff("same\nOLD\nsame2", "same\nNEW\nsame2", WIDE_ENOUGH);

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(sup_shift('y')), &mut effects);
    app.sync_view();

    assert_eq!(app.active_doc().buffer.content(), "same\nNEW\nsame2");
    let regions = app
        .diff
        .as_ref()
        .expect("diff active")
        .alignment
        .regions
        .clone();
    assert!(
        regions
            .iter()
            .all(|region| region.kind == rune_merge::RegionKind::Same),
        "the region must recompute to Same next frame: {regions:?}"
    );

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl('z')), &mut effects);
    app.sync_view();
    assert_eq!(app.active_doc().buffer.content(), "same\nOLD\nsame2");
}

#[test]
fn take_theirs_with_no_hunks_posts_a_status_instead_of_silently_doing_nothing() {
    let mut app = app_with_diff("same", "same", WIDE_ENOUGH);
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(sup_shift('y')), &mut effects);

    assert_eq!(app.active_doc().buffer.content(), "same");
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        Some("no hunk to take")
    );
}

#[test]
fn take_ours_always_reports_feedback_without_touching_the_buffer() {
    let mut app = app_with_diff("same\nOLD\nsame2", "same\nNEW\nsame2", WIDE_ENOUGH);
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(sup_shift('u')), &mut effects);

    assert_eq!(app.active_doc().buffer.content(), "same\nOLD\nsame2");
    assert_eq!(rune_tui::messages::newest_text(&app), Some("already yours"));
}

#[test]
fn hunk_navigation_wraps_and_reports_position() {
    let mut app = app_with_diff("A\nsame\nB", "X\nsame\nY", WIDE_ENOUGH);

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(sup_shift('j')), &mut effects);
    assert_eq!(app.diff.as_ref().expect("diff active").hunk_cur, 2);
    assert_eq!(rune_tui::messages::newest_text(&app), Some("hunk 2/2"));

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(sup_shift('j')), &mut effects);
    assert_eq!(app.diff.as_ref().expect("diff active").hunk_cur, 0);
    assert_eq!(rune_tui::messages::newest_text(&app), Some("hunk 1/2"));

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl('k')), &mut effects);
    assert_eq!(app.diff.as_ref().expect("diff active").hunk_cur, 2);
    assert_eq!(rune_tui::messages::newest_text(&app), Some("hunk 2/2"));
}

#[test]
fn diff_chords_are_ordinary_unbound_keys_when_no_diff_view_is_active() {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    app.frame = Some(rune_tui::app::FrameSize::new(WIDE_ENOUGH, HEIGHT));
    app.sync_view();

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(sup_shift('j')), &mut effects);

    assert_eq!(app.active_doc().buffer.content(), "hello");
    assert!(app.diff.is_none());
}

#[test]
fn a_bare_j_falls_through_to_ordinary_insertion_not_next_hunk() {
    let mut app = app_with_diff("hi", "hi", WIDE_ENOUGH);
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(key('j')), &mut effects);
    app.sync_view();
    assert_eq!(app.active_doc().buffer.content(), "jhi");
}

#[test]
fn click_in_the_left_pane_moves_the_right_pane_caret_to_the_aligned_line() {
    let app = app_with_diff("a\nb\nX\nc\nd\ne", "a\nb\nc\nd\ne", WIDE_ENOUGH);
    let geo =
        rune_tui::layout::geometry(ratatui::layout::Rect::new(0, 0, WIDE_ENOUGH, HEIGHT), &app);
    let diff_left = geo.diff_left.expect("diff pane visible at this width");
    let mut app = app;

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: diff_left.x,
            row: diff_left.y + 2,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );

    let right_content = app.active_doc().buffer.content().to_string();
    let expected = right_content.find("\nc").expect("c present in fileB") + 1;
    assert_eq!(app.active_doc().cursors.primary().position.get(), expected);
}

#[test]
fn closing_the_right_document_tears_down_the_diff_view() {
    let mut app = app_with_diff("hi", "hi", WIDE_ENOUGH);
    let right = app.active;
    assert!(app.diff.is_some());

    let mut effects = Effects::default();
    let _ = rune_tui::workspace::close_now(&mut app, right, &mut effects);

    assert!(
        app.diff.is_none(),
        "the diff view must not outlive the right document it was tracking"
    );

    let grid = testgrid::grid(&app, WIDE_ENOUGH, HEIGHT);
    assert!(
        !grid.iter().any(|row| row.contains("hi")),
        "the left pane must not still be painted after the diff view is torn down"
    );
}

#[test]
fn a_left_pane_with_no_parsed_view_yet_blanks_instead_of_leaking_the_prior_frame() {
    let mut app = app_with_diff("rightmarker", "leftmarker", WIDE_ENOUGH);
    let backend = ratatui::backend::TestBackend::new(WIDE_ENOUGH, HEIGHT);
    let mut terminal = ratatui::Terminal::new(backend).expect("construct terminal");

    terminal
        .draw(|frame| rune_tui::render::draw(&app, frame))
        .expect("draw frame 1");
    let painted = row_strings(terminal.backend().buffer(), WIDE_ENOUGH, HEIGHT);
    assert!(
        painted.iter().any(|row| row.contains("leftmarker")),
        "the left pane must render its text once parsed"
    );

    app.diff.as_mut().expect("diff active").left.view = None;
    terminal
        .draw(|frame| rune_tui::render::draw(&app, frame))
        .expect("draw frame 2");
    let after = row_strings(terminal.backend().buffer(), WIDE_ENOUGH, HEIGHT);
    assert!(
        !after.iter().any(|row| row.contains("leftmarker")),
        "an unparsed left pane must blank its rect, not leak the prior frame's text"
    );
}

const WIDE: u16 = 160;

#[test]
fn dragging_the_diff_splitter_right_widens_the_left_pane() {
    let mut app = app_with_diff("a\nb\nc", "a\nb\nc", WIDE);
    let splitter = geo(&app, WIDE)
        .diff_splitter
        .expect("diff pane visible at this width");
    let before = geo(&app, WIDE).diff_left.expect("diff pane visible");
    let before_left_w = before.width;
    let before_right_w = geo(&app, WIDE).editor.width;

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

    let after_left_w = geo(&app, WIDE).diff_left.expect("still shown").width;
    let after_right_w = geo(&app, WIDE).editor.width;
    assert_eq!(after_left_w, before_left_w + 10);
    assert_eq!(after_right_w, before_right_w - 10);
}

#[test]
fn dragging_the_diff_splitter_far_left_clamps_to_the_pane_floor() {
    let mut app = app_with_diff("a\nb\nc", "a\nb\nc", WIDE_ENOUGH);
    let splitter = geo(&app, WIDE_ENOUGH)
        .diff_splitter
        .expect("diff pane visible at this width");

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        splitter.x,
        splitter.y,
    );
    send(&mut app, MouseKind::Drag(MouseButton::Left), 0, splitter.y);
    send(&mut app, MouseKind::Up(MouseButton::Left), 0, splitter.y);

    let after = geo(&app, WIDE_ENOUGH)
        .diff_left
        .expect("non-collapsible, still shown");
    assert_eq!(after.width, rune_tui::layout::DIFF_MIN_PANE_W);
}

#[test]
fn dragging_the_diff_splitter_then_folding_and_restoring_reapplies_without_panic() {
    let mut app = app_with_diff("a\nb\nc", "a\nb\nc", WIDE_ENOUGH);
    let splitter = geo(&app, WIDE_ENOUGH)
        .diff_splitter
        .expect("diff pane visible at this width");

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
    let dragged_w = geo(&app, WIDE_ENOUGH).diff_left.expect("still shown").width;

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Resize(TOO_NARROW, HEIGHT), &mut effects);
    app.sync_view();
    assert!(geo(&app, TOO_NARROW).diff_left.is_none(), "must fold");

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Resize(WIDE_ENOUGH, HEIGHT), &mut effects);
    app.sync_view();
    let restored_w = geo(&app, WIDE_ENOUGH)
        .diff_left
        .expect("widening back restores the side-by-side pane")
        .width;
    assert_eq!(restored_w, dragged_w);
}

#[test]
fn a_drag_starting_outside_the_diff_band_still_selects_text() {
    let mut app = app_with_diff("a\nb\nc", "a\nb\nc", WIDE_ENOUGH);
    let editor = geo(&app, WIDE_ENOUGH).editor;

    send(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        editor.x,
        editor.y,
    );
    send(
        &mut app,
        MouseKind::Drag(MouseButton::Left),
        editor.x + 1,
        editor.y,
    );

    assert!(matches!(
        app.pointer.drag,
        Some(rune_tui::pointer::Drag::Text { .. })
    ));
}
