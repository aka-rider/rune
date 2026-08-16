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

#[test]
fn left_pane_lockstep_scrolls_with_the_right_pane() {
    let right_text = (0..200)
        .map(|i| format!("right line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let left_text = (0..200)
        .map(|i| format!("left line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut app = app_with_diff(&right_text, &left_text, WIDE_ENOUGH);

    for _ in 0..50 {
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

    let right_scroll = app.active_doc().viewport.scroll_row;
    assert!(right_scroll.0 > 0, "the right pane must have scrolled");
    assert_eq!(
        app.diff
            .as_ref()
            .expect("diff active")
            .left
            .viewport
            .scroll_row,
        right_scroll
    );
}
