#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use ratatui::buffer::Buffer as RtBuffer;
use ratatui::style::Modifier;

use rune_core::coords::VisualCol;
use rune_tui::app::{self, App};
use rune_tui::document::ReadOnly;
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::messages;
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{Effects, Msg};

use tui_edit_common::{ALT, HEIGHT, WIDTH, app_for, press, render_to_test_backend};

const EDITOR_TOP: usize = 2;

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

fn long_line(words: usize) -> String {
    (0..words)
        .map(|n| format!("w{n:03}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn toggle_wrap(app: &mut App) {
    press(app, KeyCode::Char('z'), ALT);
}

fn rows(buf: &RtBuffer) -> Vec<String> {
    (0..HEIGHT)
        .map(|y| {
            (0..WIDTH)
                .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                .collect()
        })
        .collect()
}

fn rows_containing(buf: &RtBuffer, needle: &str) -> usize {
    rows(buf).iter().filter(|r| r.contains(needle)).count()
}

fn caret_cells(buf: &RtBuffer) -> Vec<(u16, u16)> {
    let mut found = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if buf
                .cell((x, y))
                .is_some_and(|c| c.modifier.contains(Modifier::REVERSED))
            {
                found.push((x, y));
            }
        }
    }
    found
}

fn palette_run(app: &mut App, query: &str) {
    press(app, KeyCode::Char('p'), CTRL);
    for ch in query.chars() {
        press(app, KeyCode::Char(ch), Mods::NONE);
    }
    press(app, KeyCode::Enter, Mods::NONE);
}

#[test]
fn a_long_line_takes_one_row_with_wrap_off_and_several_with_it_on() {
    let text = format!("{}\n", long_line(60));
    let mut app = app_for(&text, 0);
    let wrapped = rows_containing(&render_to_test_backend(&app), "w0");

    toggle_wrap(&mut app);
    let unwrapped = rows_containing(&render_to_test_backend(&app), "w0");

    assert!(wrapped >= 3, "60 words must wrap over several rows");
    assert_eq!(unwrapped, 1);
}

#[test]
fn the_palette_lists_the_command_and_running_it_reports_the_new_state() {
    let mut app = app_for("hello\n", 0);
    press(&mut app, KeyCode::Char('p'), CTRL);
    for ch in "word wrap".chars() {
        press(&mut app, KeyCode::Char(ch), Mods::NONE);
    }
    assert!(
        rows_containing(&render_to_test_backend(&app), "Toggle Word Wrap") > 0,
        "the palette must list the command"
    );
    press(&mut app, KeyCode::Enter, Mods::NONE);
    assert_eq!(messages::newest_text(&app), Some("Word wrap: off"));

    palette_run(&mut app, "toggle word wrap");
    assert_eq!(messages::newest_text(&app), Some("Word wrap: on"));
}

#[test]
fn the_caret_stays_on_screen_while_the_line_scrolls_sideways_to_follow_it() {
    let text = format!("{}\n", long_line(60));
    let mut app = app_for(&text, 0);
    toggle_wrap(&mut app);

    press(&mut app, KeyCode::End, Mods::NONE);

    let buf = render_to_test_backend(&app);
    assert_eq!(caret_cells(&buf).len(), 1, "exactly one caret is painted");
    assert_eq!(rows_containing(&buf, "w059"), 1, "the line tail is shown");
    assert_eq!(
        rows_containing(&buf, "w000"),
        0,
        "the line head scrolled off"
    );

    press(&mut app, KeyCode::Home, Mods::NONE);
    let buf = render_to_test_backend(&app);
    assert_eq!(rows_containing(&buf, "w000"), 1);
    assert_eq!(app.active_doc().viewport.scroll_col, VisualCol(0));
}

#[test]
fn toggling_twice_restores_the_wrapped_picture() {
    let text = format!("{}\n", long_line(60));
    let mut app = app_for(&text, text.len() - 1);
    let before = rows(&render_to_test_backend(&app));

    toggle_wrap(&mut app);
    toggle_wrap(&mut app);

    assert_eq!(
        rows(&render_to_test_backend(&app))[EDITOR_TOP..EDITOR_TOP + 4],
        before[EDITOR_TOP..EDITOR_TOP + 4]
    );
}

#[test]
fn vertical_moves_keep_the_visual_column_across_unwrapped_lines() {
    let text = format!("{}\n{}\n", long_line(60), long_line(60));
    let mut app = app_for(&text, 0);
    toggle_wrap(&mut app);
    for _ in 0..150 {
        press(&mut app, KeyCode::Right, Mods::NONE);
    }

    press(&mut app, KeyCode::Down, Mods::NONE);

    let cursor = app.active_doc().cursors.primary();
    assert_eq!(cursor.position.get(), text.len() / 2 + 150);
    assert_eq!(cursor.desired_col, VisualCol(150));
    assert_eq!(caret_cells(&render_to_test_backend(&app)).len(), 1);
}

#[test]
fn a_click_on_a_scrolled_line_lands_on_the_character_under_the_pointer() {
    let text = format!("{}\n", long_line(60));
    let mut app = app_for(&text, 0);
    app.frame = Some(rune_tui::app::FrameSize::new(WIDTH, HEIGHT));
    toggle_wrap(&mut app);
    press(&mut app, KeyCode::End, Mods::NONE);
    let buf = render_to_test_backend(&app);
    let (caret_x, caret_y) = caret_cells(&buf)[0];

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: caret_x - 4,
            row: caret_y,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );

    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        text.len() - 1 - 4
    );
}

#[test]
fn wide_characters_cut_by_the_scroll_edge_do_not_break_the_caret() {
    let text = format!("{}\n", "\u{4f60}".repeat(100));
    let mut app = app_for(&text, 0);
    toggle_wrap(&mut app);

    for end in [true, false, true] {
        let key = if end { KeyCode::End } else { KeyCode::Home };
        press(&mut app, key, Mods::NONE);
        assert_eq!(caret_cells(&render_to_test_backend(&app)).len(), 1);
    }
}

#[test]
fn a_read_only_view_refuses_the_toggle_and_keeps_wrapping() {
    let text = format!("{}\n", long_line(60));
    let mut app = app_for(&text, 0);
    app.active_doc_mut().read_only = ReadOnly::Reading;

    toggle_wrap(&mut app);

    assert_eq!(
        messages::newest_text(&app),
        Some("a read-only view always wraps")
    );
    assert!(rows_containing(&render_to_test_backend(&app), "w0") >= 3);
}

#[test]
fn leaving_the_reading_view_brings_back_the_unwrapped_line() {
    let text = format!("{}\n", long_line(60));
    let mut app = app_for(&text, 0);
    toggle_wrap(&mut app);
    app.active_doc_mut().read_only = ReadOnly::Reading;
    app.sync_view();
    assert!(rows_containing(&render_to_test_backend(&app), "w0") >= 3);

    app.active_doc_mut().read_only = ReadOnly::No;
    app.sync_view();

    assert_eq!(rows_containing(&render_to_test_backend(&app), "w0"), 1);
}

fn wheel_down(app: &mut App, notches: usize) {
    for _ in 0..notches {
        let mut effects = Effects::default();
        app::update(
            app,
            Msg::Mouse(MouseInput {
                kind: MouseKind::ScrollDown,
                column: 10,
                row: 6,
                shift: false,
                alt: false,
                ctrl: false,
            }),
            &mut effects,
        );
    }
}

#[test]
fn toggling_wrap_after_a_wheel_scroll_leaves_the_caret_where_it_was_and_in_view() {
    let text = format!("{}\n", long_line(60)).repeat(40);
    let mut app = app_for(&text, 0);
    app.frame = Some(rune_tui::app::FrameSize::new(WIDTH, HEIGHT));
    wheel_down(&mut app, 10);
    assert_eq!(
        app.active_doc().viewport.mode,
        rune_tui::viewport::ScrollMode::Independent,
        "the wheel must leave the viewport scrolled on its own"
    );

    toggle_wrap(&mut app);

    assert_eq!(app.active_doc().cursors.primary().position.get(), 0);
    assert_eq!(caret_cells(&render_to_test_backend(&app)).len(), 1);
}

#[test]
fn the_code_background_survives_a_scroll_past_the_u16_range() {
    let text = format!("```\nshort\n{}\n```\n", "x".repeat(70_000));
    let mut app = app_for(&text, "```\nshort\n".len());
    toggle_wrap(&mut app);
    press(&mut app, KeyCode::End, Mods::NONE);
    assert!(app.active_doc().viewport.scroll_col.0 > usize::from(u16::MAX));

    let buf = render_to_test_backend(&app);

    let code_bg = app.theme.chrome.code_bg;
    let short_row = rows(&buf)
        .iter()
        .position(|row| row.contains("x"))
        .expect("the long line is on screen")
        - 1;
    let painted = (1..WIDTH - 1)
        .filter(|&x| buf.cell((x, short_row as u16)).unwrap().style().bg == Some(code_bg))
        .count();
    assert!(
        painted >= 70,
        "the short line's row must stay painted to the right edge"
    );
}
