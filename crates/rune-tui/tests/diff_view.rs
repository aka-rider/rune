#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::diff_view;
use rune_tui::document::ReadOnly;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

const HEIGHT: u16 = 24;
const WIDE_ENOUGH: u16 = 83;
const TOO_NARROW: u16 = 82;

fn app_with_diff(right_text: &str, left_text: &str, width: u16) -> App {
    let mut app = App::new(Buffer::new(right_text), None, Arc::new(Mem::new()), None);
    diff_view::install(
        &mut app,
        left_text.as_bytes().to_vec(),
        "fileA.md".to_string(),
    )
    .expect("fixture is valid UTF-8");
    app.frame_width = width;
    app.frame_height = HEIGHT;
    app.sync_view();
    app
}

fn key(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods::NONE,
    }
}

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
fn narrow_width_hides_the_left_pane_and_editor_spans_the_center() {
    let app = app_with_diff("rightmarker", "leftmarker", TOO_NARROW);
    let grid = testgrid::grid(&app, TOO_NARROW, HEIGHT);
    let joined = grid.join("\n");

    assert!(joined.contains("rightmarker"), "fileB must still render");
    assert!(
        !joined.contains("leftmarker"),
        "fileA must not render when the center is too narrow for both panes"
    );
    assert_eq!(
        rune_tui::layout::geometry(ratatui::layout::Rect::new(0, 0, TOO_NARROW, HEIGHT), &app)
            .diff_left,
        None
    );
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

fn char_slice(row: &str, width: usize) -> String {
    row.chars().take(width).collect()
}

fn col_of(row: &str, needle: &str) -> u16 {
    let byte_idx = row.find(needle).expect("needle present in row");
    row[..byte_idx].chars().count() as u16
}

#[test]
fn right_pane_insertion_shows_a_left_filler_row_at_the_aligned_position() {
    let app = app_with_diff("a\nb\nX\nc", "a\nb\nc", WIDE_ENOUGH);
    let grid = testgrid::grid(&app, WIDE_ENOUGH, HEIGHT);

    let x_row = grid
        .iter()
        .position(|row| row.contains('X'))
        .expect("the inserted right-only line renders");
    let left_half = char_slice(&grid[x_row], 40);
    assert!(
        left_half.contains('╌'),
        "the left pane must show a filler row aligned with the right-only insertion: {left_half:?}"
    );
    assert!(!left_half.contains('a'));
    assert!(!left_half.contains('b'));
    assert!(!left_half.contains('c'));
}

#[test]
fn changed_regions_carry_the_correct_per_side_backgrounds() {
    let app = app_with_diff("same\nNEW\nsame2", "same\nOLD\nsame2", WIDE_ENOUGH);
    let grid = testgrid::grid(&app, WIDE_ENOUGH, HEIGHT);
    let buf = testgrid::draw(&app, WIDE_ENOUGH, HEIGHT);

    let old_row = grid
        .iter()
        .position(|row| row.contains("OLD"))
        .expect("the left changed line renders");
    let old_col = col_of(&grid[old_row], "OLD");
    let old_bg = buf
        .cell((old_col, old_row as u16))
        .and_then(|c| c.style().bg);
    assert_eq!(old_bg, app.theme.chrome.merge_theirs_bg.bg);

    let new_row = grid
        .iter()
        .position(|row| row.contains("NEW"))
        .expect("the right changed line renders");
    let new_col = col_of(&grid[new_row], "NEW");
    let new_bg = buf
        .cell((new_col, new_row as u16))
        .and_then(|c| c.style().bg);
    assert_eq!(new_bg, app.theme.chrome.merge_ours_bg.bg);
}

#[test]
fn an_edit_recomputes_alignment_within_the_same_settle_pass() {
    let mut app = app_with_diff("a\nb", "a\nb", WIDE_ENOUGH);
    let mut effects = Effects::default();

    app::update(&mut app, Msg::Key(key('!')), &mut effects);
    app.sync_view();

    let buf = testgrid::draw(&app, WIDE_ENOUGH, HEIGHT);
    let grid = testgrid::grid(&app, WIDE_ENOUGH, HEIGHT);
    let row = grid
        .iter()
        .position(|row| row.contains("!a"))
        .expect("the edited line renders");
    let col = col_of(&grid[row], "!");
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
