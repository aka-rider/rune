#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod diff_view_common;

use std::sync::Arc;

use diff_view_common::{
    HEIGHT, TOO_NARROW, WIDE_ENOUGH, app_with_diff, char_slice_from, col_of, col_of_right_pane,
    key, sup_shift,
};
use rune_core::buffer::Buffer;
use rune_merge::RegionKind;
use rune_tui::app::{self, App};
use rune_tui::diff_view;
use rune_tui::document::ReadOnly;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

#[test]
fn wide_enough_renders_file_a_left_of_file_b() {
    let app = app_with_diff("rightmarker", "leftmarker", WIDE_ENOUGH);
    let grid = testgrid::grid(&app, WIDE_ENOUGH, HEIGHT);

    let left_row = grid
        .iter()
        .find(|row| row.contains("leftmarker"))
        .expect("fileA's text must render somewhere on screen");
    let right_row = grid
        .iter()
        .find(|row| row.contains("rightmarker"))
        .expect("fileB's text must render somewhere on screen");

    let left_col = left_row.find("leftmarker").expect("found above");
    let right_col = right_row.find("rightmarker").expect("found above");
    assert!(
        left_col < right_col,
        "fileA must render to the left of fileB: left at {left_col}, right at {right_col}"
    );
}

#[test]
fn narrow_width_folds_the_left_pane_below_the_right_document() {
    let app = app_with_diff("rightmarker", "leftmarker", TOO_NARROW);
    let grid = testgrid::grid(&app, TOO_NARROW, HEIGHT);
    let joined = grid.join("\n");

    assert!(joined.contains("rightmarker"), "fileB must still render");

    let right_row = grid
        .iter()
        .position(|row| row.contains("rightmarker"))
        .expect("fileB's text must render somewhere on screen");
    let left_row = grid
        .iter()
        .position(|row| row.contains("leftmarker"))
        .expect("fileA's text must fold in as a virtual row when the panes can't sit side by side");
    assert!(
        left_row > right_row,
        "the virtual theirs row must render below its region's right rows: right at {right_row}, left at {left_row}"
    );

    assert_eq!(
        rune_tui::layout::geometry(ratatui::layout::Rect::new(0, 0, TOO_NARROW, HEIGHT), &app)
            .diff_left,
        None
    );
}

#[test]
fn typing_while_folded_edits_the_buffer_at_correct_offsets() {
    let mut app = app_with_diff("a\nb\nc", "a\nX\nc", TOO_NARROW);
    let mut effects = Effects::default();

    app::update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Down,
            mods: Mods::NONE,
        }),
        &mut effects,
    );
    app::update(&mut app, Msg::Key(key('!')), &mut effects);
    app.sync_view();

    assert_eq!(app.active_doc().buffer.content(), "a\n!b\nc");
}

#[test]
fn resize_round_trip_preserves_buffer_hunk_cursor_and_focus() {
    use rune_tui::pane::Pane;

    let mut app = app_with_diff("a\nb\nc", "a\nX\nc", WIDE_ENOUGH);
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(sup_shift('j')), &mut effects);
    app.sync_view();

    let content_before = app.active_doc().buffer.content().to_string();
    let hunk_before = app.diff.as_ref().expect("diff active").hunk_cur;
    assert_eq!(app.focus(), Pane::Editor);

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Resize(TOO_NARROW, HEIGHT), &mut effects);
    app.sync_view();
    assert_eq!(
        rune_tui::layout::geometry(ratatui::layout::Rect::new(0, 0, TOO_NARROW, HEIGHT), &app)
            .diff_left,
        None,
        "the narrow resize must fold"
    );
    assert_eq!(app.active_doc().buffer.content(), content_before);
    assert_eq!(
        app.diff.as_ref().expect("diff active").hunk_cur,
        hunk_before
    );
    assert_eq!(app.focus(), Pane::Editor);

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Resize(WIDE_ENOUGH, HEIGHT), &mut effects);
    app.sync_view();
    assert!(
        rune_tui::layout::geometry(ratatui::layout::Rect::new(0, 0, WIDE_ENOUGH, HEIGHT), &app)
            .diff_left
            .is_some(),
        "widening back must restore the side-by-side pane"
    );
    assert_eq!(app.active_doc().buffer.content(), content_before);
    assert_eq!(
        app.diff.as_ref().expect("diff active").hunk_cur,
        hunk_before
    );
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn typing_edits_file_b_only() {
    let mut app = app_with_diff("hello", "left original", WIDE_ENOUGH);
    let mut effects = Effects::default();

    app::update(&mut app, Msg::Key(key('!')), &mut effects);
    app.sync_view();

    assert_eq!(app.active_doc().buffer.content(), "!hello");
    assert_eq!(
        app.diff
            .as_ref()
            .expect("diff active")
            .left
            .buffer
            .content(),
        "left original"
    );
}

#[test]
fn the_left_document_is_read_only() {
    let app = app_with_diff("hello", "left original", WIDE_ENOUGH);
    assert_eq!(
        app.diff.as_ref().expect("diff active").left.read_only,
        ReadOnly::Always
    );
}

#[test]
fn install_refuses_invalid_utf8() {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    let err = diff_view::install(&mut app, vec![0xff, 0xfe], "fileA.md".to_string())
        .expect_err("invalid UTF-8 must be refused");
    assert_eq!(err, diff_view::DiffInstallError::InvalidUtf8);
    assert!(app.diff.is_none());
}

#[test]
fn right_pane_insertion_shows_a_left_filler_row_at_the_aligned_position() {
    let app = app_with_diff("a\nb\nX\nc", "a\nb\nc", WIDE_ENOUGH);
    let grid = testgrid::grid(&app, WIDE_ENOUGH, HEIGHT);
    let geo =
        rune_tui::layout::geometry(ratatui::layout::Rect::new(0, 0, WIDE_ENOUGH, HEIGHT), &app);
    let diff_left = geo.diff_left.expect("diff pane visible at this width");

    let x_row = grid
        .iter()
        .position(|row| row.contains('X'))
        .expect("the inserted right-only line renders");
    let left_half = char_slice_from(&grid[x_row], diff_left.x as usize, diff_left.width as usize);
    assert!(
        left_half.trim().is_empty(),
        "the left pane must show a blank filler row aligned with the right-only insertion: {left_half:?}"
    );
}

#[test]
fn default_split_is_even() {
    let app = app_with_diff("a\nb\nc", "a\nb\nc", 160);
    let geo = rune_tui::layout::geometry(ratatui::layout::Rect::new(0, 0, 160, HEIGHT), &app);
    let diff_left = geo.diff_left.expect("diff pane visible at this width");
    let right_width = geo.editor.width;
    assert!(
        diff_left.width.abs_diff(right_width) <= 1,
        "left {} and right {right_width} must differ by at most 1 column",
        diff_left.width
    );
}

#[test]
fn changed_regions_carry_the_correct_per_side_backgrounds() {
    let app = app_with_diff(
        "same\nthe NEW word\nsame2",
        "same\nthe OLD word\nsame2",
        WIDE_ENOUGH,
    );
    let grid = testgrid::grid(&app, WIDE_ENOUGH, HEIGHT);
    let buf = testgrid::draw(&app, WIDE_ENOUGH, HEIGHT);

    let old_row = grid
        .iter()
        .position(|row| row.contains("OLD"))
        .expect("the left changed line renders");
    let old_col = col_of(&grid[old_row], "word");
    let old_bg = buf
        .cell((old_col, old_row as u16))
        .and_then(|c| c.style().bg);
    assert_eq!(old_bg, app.theme.chrome.merge_theirs_bg.bg);

    let new_row = grid
        .iter()
        .position(|row| row.contains("NEW"))
        .expect("the right changed line renders");
    let new_col = col_of_right_pane(&grid[new_row], "word");
    let new_bg = buf
        .cell((new_col, new_row as u16))
        .and_then(|c| c.style().bg);
    assert_eq!(new_bg, app.theme.chrome.merge_ours_bg.bg);
}

#[test]
fn an_edit_recomputes_alignment_within_the_same_settle_pass() {
    let mut app = app_with_diff("a same\nb", "a same\nb", WIDE_ENOUGH);
    let mut effects = Effects::default();

    app::update(&mut app, Msg::Key(key('!')), &mut effects);
    app.sync_view();

    let buf = testgrid::draw(&app, WIDE_ENOUGH, HEIGHT);
    let grid = testgrid::grid(&app, WIDE_ENOUGH, HEIGHT);
    let row = grid
        .iter()
        .position(|row| row.contains("!a"))
        .expect("the edited line renders");
    let col = col_of_right_pane(&grid[row], "same");
    let bg = buf.cell((col, row as u16)).and_then(|c| c.style().bg);
    assert_eq!(
        bg, app.theme.chrome.merge_ours_bg.bg,
        "the very next frame after the edit must already carry the recomputed alignment"
    );
}

#[test]
fn scrolling_the_right_pane_scrolls_the_left_pane_to_the_aligned_row() {
    let mut right_lines: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
    right_lines.push("INSERTED".to_string());
    right_lines.extend((50..100).map(|i| format!("line {i}")));
    let right_text = right_lines.join("\n");
    let left_text = (0..100)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut app = app_with_diff(&right_text, &left_text, WIDE_ENOUGH);

    for _ in 0..70 {
        let mut effects = Effects::default();
        app::update(
            &mut app,
            Msg::Key(KeyInput {
                code: KeyCode::Down,
                mods: Mods::NONE,
            }),
            &mut effects,
        );
    }
    app.sync_view();

    let right_scroll = app.active_doc().viewport.scroll_row.0;
    assert!(
        right_scroll >= 51,
        "the right pane must scroll past the inserted line"
    );
    let left_scroll = app
        .diff
        .as_ref()
        .expect("diff active")
        .left
        .viewport
        .scroll_row
        .0;
    assert_eq!(
        left_scroll,
        right_scroll - 1,
        "the left pane must scroll to the row aligned with the right pane, not lockstep"
    );
}

#[test]
fn undo_of_an_edit_restores_the_previous_alignment() {
    let mut app = app_with_diff("a\nb", "a\nb", WIDE_ENOUGH);
    let mut effects = Effects::default();

    app::update(&mut app, Msg::Key(key('!')), &mut effects);
    app.sync_view();
    assert_eq!(app.active_doc().buffer.content(), "!a\nb");

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('z'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );
    app.sync_view();
    assert_eq!(app.active_doc().buffer.content(), "a\nb");

    let buf = testgrid::draw(&app, WIDE_ENOUGH, HEIGHT);
    let grid = testgrid::grid(&app, WIDE_ENOUGH, HEIGHT);
    let row = grid
        .iter()
        .position(|row| row.contains('a'))
        .expect("the reverted line renders");
    let col = col_of(&grid[row], "a");
    let bg = buf.cell((col, row as u16)).and_then(|c| c.style().bg);
    assert_ne!(
        bg, app.theme.chrome.merge_ours_bg.bg,
        "undo must recompute alignment back to Same, clearing the changed background"
    );
}

#[test]
fn a_one_word_change_is_emphasized_on_exactly_that_word_in_both_panes() {
    let app = app_with_diff(
        "same\nthe dog sat\nsame2",
        "same\nthe cat sat\nsame2",
        WIDE_ENOUGH,
    );
    let grid = testgrid::grid(&app, WIDE_ENOUGH, HEIGHT);
    let buf = testgrid::draw(&app, WIDE_ENOUGH, HEIGHT);

    let right_row = grid
        .iter()
        .position(|row| row.contains("dog"))
        .expect("the changed right line renders");
    let dog_col = col_of(&grid[right_row], "dog");
    let dog_bg = buf
        .cell((dog_col, right_row as u16))
        .and_then(|c| c.style().bg);
    assert_eq!(dog_bg, app.theme.chrome.diff_word_ours.bg);

    let sat_col = col_of_right_pane(&grid[right_row], "sat");
    let sat_bg = buf
        .cell((sat_col, right_row as u16))
        .and_then(|c| c.style().bg);
    assert_eq!(
        sat_bg, app.theme.chrome.merge_ours_bg.bg,
        "an unchanged word in the same region must carry the region background, not the word emphasis"
    );

    let left_row = grid
        .iter()
        .position(|row| row.contains("cat"))
        .expect("the changed left line renders");
    let cat_col = col_of(&grid[left_row], "cat");
    let cat_bg = buf
        .cell((cat_col, left_row as u16))
        .and_then(|c| c.style().bg);
    assert_eq!(cat_bg, app.theme.chrome.diff_word_theirs.bg);
}

#[test]
fn regions_outside_the_visible_row_range_are_not_computed() {
    let mut right_lines: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
    let mut left_lines = right_lines.clone();
    right_lines[50] = "line fifty changed".to_string();
    left_lines[50] = "line fifty original".to_string();
    let right_text = right_lines.join("\n");
    let left_text = left_lines.join("\n");

    let app = app_with_diff(&right_text, &left_text, WIDE_ENOUGH);
    let diff = app.diff.as_ref().expect("diff active");

    assert!(
        diff.alignment
            .regions
            .iter()
            .any(|r| r.kind == RegionKind::Changed),
        "the off-screen line must still produce a Changed region"
    );
    assert!(
        diff.intraline_left.is_empty(),
        "an off-screen region must not have its intraline spans computed"
    );
    assert!(
        diff.intraline_right.is_empty(),
        "an off-screen region must not have its intraline spans computed"
    );
}
