#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use crate::app::App;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::pane::Pane;
use crate::runtime::{Effects, Msg};

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};
const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};

fn app() -> App {
    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    app.frame = Some(crate::app::FrameSize::new(120, 34));
    app
}

fn key(app: &mut App, code: KeyCode, mods: Mods, effects: &mut Effects) {
    crate::app::update(app, Msg::Key(KeyInput { code, mods }), effects);
}

#[test]
fn ctrl_shift_f_chord_opens_project_search_on_a_never_shown_left_column() {
    let mut app = app();
    assert!(!app.splits.left.is_shown(), "test setup: column hidden");
    let mut effects = Effects::default();

    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);

    assert!(app.projectsearch().is_some());
    assert_eq!(app.focus(), Pane::Explorer);
}

#[test]
fn sup_shift_f_chord_again_closes_and_restores_return_to() {
    let mut app = app();
    let second = app.open_document(Buffer::new("second"));
    crate::workspace::switch_to(&mut app, second);
    let mut effects = Effects::default();

    key(&mut app, KeyCode::Char('F'), SUP, &mut effects);
    assert!(app.projectsearch().is_some(), "test setup: panel open");

    key(&mut app, KeyCode::Char('F'), SUP, &mut effects);

    assert!(app.projectsearch().is_none());
    assert_eq!(app.active, second);
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn escape_closes_and_restores_return_to() {
    let mut app = app();
    let second = app.open_document(Buffer::new("second"));
    crate::workspace::switch_to(&mut app, second);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    assert!(app.projectsearch().is_some(), "test setup: panel open");

    key(&mut app, KeyCode::Escape, Mods::NONE, &mut effects);

    assert!(app.projectsearch().is_none());
    assert_eq!(app.active, second);
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn typed_chars_echo_in_the_query_without_reminting_the_generation() {
    let mut app = app();
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    let minted = app.projectsearch().expect("open").query_generation;

    key(&mut app, KeyCode::Char('h'), Mods::NONE, &mut effects);
    key(&mut app, KeyCode::Char('i'), Mods::NONE, &mut effects);
    key(&mut app, KeyCode::Backspace, Mods::NONE, &mut effects);

    let state = app.projectsearch().expect("still open");
    assert_eq!(state.query, "h");
    assert_eq!(state.query_generation, minted);
    assert_eq!(
        app.active_doc().buffer.content(),
        "hello",
        "typing into the panel must never reach the editor"
    );
}

#[test]
fn opening_over_the_file_finder_tears_it_down_through_cancel() {
    let mut app = app();
    let second = app.open_document(Buffer::new("second"));
    crate::workspace::switch_to(&mut app, second);
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('o'), SUP, &mut effects);
    assert!(app.filesearch().is_some(), "test setup: finder open");

    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);

    assert!(app.filesearch().is_none());
    assert!(app.projectsearch().is_some());
    assert_eq!(
        app.active, second,
        "the finder's return_to must be restored before the panel records its own"
    );
    assert_eq!(app.focus(), Pane::Explorer);
}

#[test]
fn a_close_bars_global_closes_project_search_through_close_all_overlays() {
    let mut app = app();
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);
    assert!(app.projectsearch().is_some(), "test setup: panel open");

    key(&mut app, KeyCode::F1, Mods::NONE, &mut effects);

    assert!(app.projectsearch().is_none());
}

#[test]
fn a_paste_lands_in_the_query_not_the_editor() {
    let mut app = app();
    let mut effects = Effects::default();
    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);

    crate::app::update(
        &mut app,
        Msg::Paste("grep\nsecond line".to_string()),
        &mut effects,
    );

    assert_eq!(
        app.projectsearch().map(|s| s.query.as_str()),
        Some("grep"),
        "only the first pasted line survives sanitization"
    );
    assert_eq!(app.active_doc().buffer.content(), "hello");
}
