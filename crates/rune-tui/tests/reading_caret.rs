#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::coords::DisplayRow;
use rune_tui::app::{self, App};
use rune_tui::document::ReadOnly;
use rune_tui::footer;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_basic(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn plain(code: KeyCode) -> Msg {
    Msg::Key(KeyInput {
        code,
        mods: Mods::NONE,
    })
}

fn ctrl(c: char) -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char(c),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

fn send(app: &mut App, msg: Msg) {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
}

fn enter_reading(app: &mut App) {
    send(app, ctrl('P'));
    assert_eq!(app.active_doc().read_only, ReadOnly::Reading);
}

#[test]
fn scrolling_in_reading_view_moves_the_caret_with_the_view() {
    let content: String = (0..50).map(|i| format!("line {i}\n")).collect();
    let mut app = app_basic(&content);
    enter_reading(&mut app);

    for _ in 0..5 {
        send(&mut app, plain(KeyCode::Down));
    }

    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(5));
    let expected = app
        .active_doc()
        .buffer
        .line_start(5)
        .expect("line 5 exists in a 50-line fixture");
    assert_eq!(app.active_doc().cursors.primary().position.get(), expected);
}

#[test]
fn footer_position_text_reports_the_first_visible_line_while_scrolling() {
    let content: String = (0..50).map(|i| format!("line {i}\n")).collect();
    let mut app = app_basic(&content);
    enter_reading(&mut app);

    for _ in 0..7 {
        send(&mut app, plain(KeyCode::Down));
    }

    assert_eq!(footer::position_text(&app).as_deref(), Some("Ln 8, Col 1"));
}

#[test]
fn following_a_link_after_scrolling_targets_a_link_on_screen() {
    let mut lines = vec!["[link one](#Section-A)".to_string()];
    for i in 1..=10 {
        lines.push(format!("filler {i}"));
    }
    lines.push("[link two](#Section-B)".to_string());
    lines.push(String::new());
    lines.push("## Section-A".to_string());
    lines.push("body a".to_string());
    lines.push(String::new());
    lines.push("## Section-B".to_string());
    lines.push("body b".to_string());
    let content = lines.join("\n") + "\n";
    let link_two_line = 11;

    let mut app = app_basic(&content);
    enter_reading(&mut app);

    for _ in 0..link_two_line {
        send(&mut app, plain(KeyCode::Down));
    }
    assert_eq!(
        app.active_doc().viewport.scroll_row,
        DisplayRow(link_two_line)
    );

    send(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Enter,
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        }),
    );

    let section_b = content
        .find("## Section-B")
        .expect("fixture has a Section-B heading");
    let section_a = content
        .find("## Section-A")
        .expect("fixture has a Section-A heading");
    let landed = app.active_doc().cursors.primary().position.get();
    assert_eq!(
        landed, section_b,
        "the on-screen link (link two) must be the one followed"
    );
    assert_ne!(
        landed, section_a,
        "the scrolled-away link (link one) must not be the one followed"
    );
}

#[test]
fn leaving_reading_view_does_not_move_the_caret() {
    let content: String = (0..50).map(|i| format!("line {i}\n")).collect();
    let mut app = app_basic(&content);
    enter_reading(&mut app);

    for _ in 0..5 {
        send(&mut app, plain(KeyCode::Down));
    }
    let caret_while_reading = app.active_doc().cursors.primary().position;

    send(&mut app, ctrl('P'));

    assert_eq!(app.active_doc().read_only, ReadOnly::No);
    assert_eq!(
        app.active_doc().cursors.primary().position,
        caret_while_reading,
        "leaving reading view must not move the caret"
    );
}
