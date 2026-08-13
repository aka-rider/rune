#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod explorer_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs};

const WIDTH: u16 = 80;
const HEIGHT: u16 = 30;

fn app_with(mem: &Arc<Mem>, path: &str, content: &str) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new(content), Some(PathBuf::from(path)), vfs, None);
    app.set_root(PathBuf::from("/root"));
    app.frame_width = WIDTH;
    app.frame_height = HEIGHT;
    app.sync_view();
    app
}

fn plain_app(content: &str, width: u16, height: u16) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.frame_width = width;
    app.frame_height = height;
    app.sync_view();
    app
}

fn place_cursor(app: &mut App, offset: usize) {
    app.active_doc_mut().cursors = CursorSet::new(offset);
}

fn modded(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

fn ctrl(code: KeyCode) -> KeyInput {
    modded(
        code,
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    )
}

fn plain(code: KeyCode) -> KeyInput {
    modded(code, Mods::NONE)
}

fn sup_enter() -> KeyInput {
    modded(
        KeyCode::Enter,
        Mods {
            sup: true,
            ..Mods::NONE
        },
    )
}

fn back_key() -> KeyInput {
    ctrl(KeyCode::Char('['))
}

fn forward_key() -> KeyInput {
    ctrl(KeyCode::Char(']'))
}

fn press(app: &mut App, key: KeyInput) -> Effects {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(key), &mut effects);
    app.sync_view();
    effects
}

fn settle_file_opens(app: &mut App, mut effects: Effects) {
    for cmd in effects.cmds.drain(..) {
        assert_eq!(cmd.kind(), CmdKind::ReadFile);
        if let Some(msg) = cmd.run() {
            let mut inner = Effects::default();
            app::update(app, msg, &mut inner);
        }
    }
    app.sync_view();
}

fn press_and_open(app: &mut App, key: KeyInput) {
    let effects = press(app, key);
    settle_file_opens(app, effects);
}

fn editor_origin(app: &App) -> (u16, u16) {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let editor = rune_tui::layout::geometry(area, app).editor;
    (editor.x, editor.y)
}

fn click(app: &mut App, col: u16, row: u16) {
    let (ox, oy) = editor_origin(app);
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: ox + col,
            row: oy + row,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
    app.sync_view();
}

fn numbered_lines(count: usize) -> String {
    (0..count).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn link_follow_then_back_returns_and_forward_returns_again() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/note.md"), b"note body\n")
        .expect("seed note.md");
    let content = "[[note]]\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    let link_offset = content.find("note").expect("fixture has note");
    place_cursor(&mut app, link_offset);

    press_and_open(&mut app, sup_enter());
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/note.md"))
    );

    press(&mut app, back_key());
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/a.md"))
    );
    assert_eq!(app.active_doc().cursors.primary().position, link_offset);

    press(&mut app, forward_key());
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/note.md"))
    );
}

#[test]
fn search_jump_then_back_returns_to_the_starting_caret() {
    let mut content = String::from("start\n");
    for i in 0..14 {
        content.push_str(&format!("filler {i}\n"));
    }
    content.push_str("NEEDLE ahead\n");
    let mem = Arc::new(Mem::new());
    let mut app = app_with(&mem, "/root/a.md", &content);
    let start_offset = app.active_doc().cursors.primary().position;

    press(&mut app, ctrl(KeyCode::Char('f')));
    for c in "NEEDLE".chars() {
        press(&mut app, plain(KeyCode::Char(c)));
    }
    press(&mut app, plain(KeyCode::Enter));
    let jumped = app.active_doc().cursors.primary().position;
    assert!(jumped > start_offset, "the search jump must move the caret");

    press(&mut app, back_key());
    assert_eq!(app.active_doc().cursors.primary().position, start_offset);
}

#[test]
fn arrowing_the_explorer_preview_records_nothing() {
    let mem = explorer_common::seeded_vfs();
    let mut app = explorer_common::app_with(&mem);
    explorer_common::load_explorer(&mut app);

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Key(explorer_common::key(KeyCode::Down)),
        &mut effects,
    );
    settle_file_opens(&mut app, effects);
    assert!(app.active_doc().is_preview());
    assert_eq!(app.nav_history.len(), 0);

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Key(explorer_common::key(KeyCode::Down)),
        &mut effects,
    );
    settle_file_opens(&mut app, effects);
    assert_eq!(app.nav_history.len(), 0);
}

#[test]
fn ctrl_1_tab_switch_records_nothing() {
    let mem = Arc::new(Mem::new());
    let mut app = app_with(&mem, "/root/a.md", "a content\n");
    let a_id = app.active;
    let b_id = app.open_document(Buffer::new("b content\n"));
    rune_tui::workspace::switch_to(&mut app, b_id);
    assert_eq!(app.active, b_id);

    press(&mut app, ctrl(KeyCode::Char('1')));

    assert_eq!(app.active, a_id);
    assert_eq!(app.nav_history.len(), 0);
}

#[test]
fn five_consecutive_backs_never_grow_the_entry_list() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/note.md"), b"note body\n")
        .expect("seed note.md");
    let content = "[[note]]\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    place_cursor(&mut app, content.find("note").expect("fixture has note"));
    press_and_open(&mut app, sup_enter());

    press(&mut app, back_key());
    let stable_len = app.nav_history.len();
    assert!(stable_len >= 1);

    for _ in 0..4 {
        press(&mut app, back_key());
        assert_eq!(
            app.nav_history.len(),
            stable_len,
            "further backs must never grow the entry list"
        );
    }
}

#[test]
fn a_thirty_line_jump_records_while_a_five_line_move_does_not() {
    let content = numbered_lines(40);
    let mut app = plain_app(&content, WIDTH, 50);
    place_cursor(&mut app, 0);

    click(&mut app, 0, 5);
    assert_eq!(app.nav_history.len(), 0);

    click(&mut app, 0, 35);
    assert_eq!(app.nav_history.len(), 1);
}

#[test]
fn an_entry_below_an_insertion_still_lands_on_the_same_text() {
    let mut lines: Vec<String> = (0..20).map(|i| format!("line {i}\n")).collect();
    lines[15] = "TARGET\n".to_string();
    let content: String = lines.concat();
    let target_offset = content.find("TARGET").expect("fixture has TARGET");
    let mut app = plain_app(&content, WIDTH, 40);
    place_cursor(&mut app, target_offset);

    click(&mut app, 0, 0);
    assert_eq!(app.nav_history.len(), 1);

    press(&mut app, plain(KeyCode::Char('X')));

    press(&mut app, back_key());
    press(&mut app, back_key());

    let landed = app.active_doc().cursors.primary().position;
    let buffer = app.active_doc().buffer.content();
    assert_eq!(&buffer[landed..landed + "TARGET".len()], "TARGET");
}

#[test]
fn back_with_empty_history_posts_a_message_and_moves_nothing() {
    let mut app = plain_app("hello\n", WIDTH, HEIGHT);
    let before = app.active;
    let cursor_before = app.active_doc().cursors.primary().position;

    press(&mut app, back_key());

    assert_eq!(
        rune_tui::messages::newest_text(&app),
        Some("no earlier location")
    );
    assert_eq!(app.active, before);
    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
    assert_eq!(app.nav_history.index(), 0);
    assert_eq!(app.nav_history.len(), 0);
}
