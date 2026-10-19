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

    let cursor = app.active_doc().cursors().primary();
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
        app.active_doc().cursors().primary().position.get(),
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

    assert_eq!(app.active_doc().cursors().primary().position.get(), 0);
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

fn table_rule_and_long_line() -> (String, usize) {
    let a = "a".repeat(45);
    let b = "b".repeat(45);
    let head = format!("| {a} | {b} |\n|---|---|\n| 1 | 2 |\n\n---\n\n");
    let caret = head.len();
    (format!("{head}{}\n", long_line(24)), caret)
}

fn rule_rows(buf: &RtBuffer) -> Vec<String> {
    rows(buf)
        .into_iter()
        .skip(EDITOR_TOP)
        .take(8)
        .filter(|r| r.contains('\u{2500}') && r.trim_matches(['\u{2502}', '\u{2500}']).is_empty())
        .collect()
}

#[test]
fn a_horizontal_rule_keeps_spanning_the_viewport_while_the_text_scrolls() {
    let (text, caret) = table_rule_and_long_line();
    let mut app = app_for(&text, caret);
    toggle_wrap(&mut app);
    let before = rule_rows(&render_to_test_backend(&app));

    press(&mut app, KeyCode::End, Mods::NONE);

    assert!(app.active_doc().viewport.scroll_col.0 > 0);
    assert_eq!(before.len(), 1, "the rule is on screen");
    assert_eq!(rule_rows(&render_to_test_backend(&app)), before);
}

#[test]
fn a_table_lays_out_at_its_natural_width_and_scrolls_with_the_text() {
    let (text, caret) = table_rule_and_long_line();
    let mut app = app_for(&text, caret);
    toggle_wrap(&mut app);
    let before = render_to_test_backend(&app);
    assert_eq!(
        rows_containing(&before, "\u{2510}"),
        0,
        "the table is wider than the viewport, so its right corner is off screen"
    );

    press(&mut app, KeyCode::End, Mods::NONE);

    let after = rows(&render_to_test_backend(&app));
    let top = after.iter().find(|r| r.contains('\u{2510}')).unwrap();
    let bottom = after.iter().find(|r| r.contains('\u{2518}')).unwrap();
    let corner_col = |row: &str, glyph: char| row.chars().position(|c| c == glyph);
    assert_eq!(
        corner_col(top, '\u{2510}'),
        corner_col(bottom, '\u{2518}'),
        "borders stay aligned with each other"
    );
    assert_eq!(
        rows_containing(&render_to_test_backend(&app), &"b".repeat(45)),
        1,
        "the second column is whole, not wrapped"
    );
}

#[test]
fn a_diff_view_refuses_the_toggle_and_keeps_both_sides_wrapping() {
    let text = format!("{}\n", long_line(30));
    let mut app = app_for(&text, 0);
    app.frame = Some(rune_tui::app::FrameSize::new(WIDTH, HEIGHT));
    rune_tui::diff_view::install(&mut app, text.clone().into_bytes(), "a.md".into()).unwrap();
    app.sync_view();
    let wrapped = rows_containing(&render_to_test_backend(&app), "w0");

    toggle_wrap(&mut app);

    assert_eq!(
        messages::newest_text(&app),
        Some("a diff view always wraps")
    );
    assert_eq!(
        rows_containing(&render_to_test_backend(&app), "w0"),
        wrapped
    );
}

#[test]
fn opening_a_diff_over_an_unwrapped_document_wraps_it_and_closing_restores_the_choice() {
    let text = format!("{}\n", long_line(30));
    let mut app = app_for(&text, 0);
    app.frame = Some(rune_tui::app::FrameSize::new(WIDTH, HEIGHT));
    toggle_wrap(&mut app);
    assert_eq!(rows_containing(&render_to_test_backend(&app), "w0"), 1);

    rune_tui::diff_view::install(&mut app, text.clone().into_bytes(), "a.md".into()).unwrap();
    app.sync_view();
    assert!(rows_containing(&render_to_test_backend(&app), "w0") >= 2);

    let id = app.active;
    rune_tui::diff_view::teardown(&mut app, id);
    app.sync_view();
    assert_eq!(rows_containing(&render_to_test_backend(&app), "w0"), 1);
}

#[test]
fn narrowing_the_window_while_scrolled_sideways_keeps_the_caret_in_view() {
    let text = format!("{}\n", long_line(60));
    let mut app = app_for(&text, 0);
    app.frame = Some(rune_tui::app::FrameSize::new(WIDTH, HEIGHT));
    toggle_wrap(&mut app);
    press(&mut app, KeyCode::End, Mods::NONE);
    let scrolled = app.active_doc().viewport.scroll_col;
    assert!(scrolled.0 > 0);

    let narrow = 40;
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Resize(narrow, HEIGHT), &mut effects);
    app.sync_view();

    assert!(app.active_doc().viewport.scroll_col > scrolled);
    let buf = rune_tui::testgrid::draw(&app, narrow, HEIGHT);
    let carets = (0..narrow)
        .flat_map(|x| (0..HEIGHT).map(move |y| (x, y)))
        .filter(|&pos| {
            buf.cell(pos)
                .is_some_and(|c| c.modifier.contains(Modifier::REVERSED))
        })
        .count();
    assert_eq!(carets, 1);
}
