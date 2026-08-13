//! Shared fixtures for the two location-history suites: `navhistory.rs`
//! (the caret rules and travel itself) and `navhistory_browse.rs` (the
//! departure every deliberate navigation records). Integration test files
//! are separate binaries, so this is the one place both draw an identical
//! `App`/`Mem` fixture from.
#![allow(dead_code)]

#[path = "../explorer_common/mod.rs"]
pub mod explorer_common;

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs};

pub const WIDTH: u16 = 80;
pub const HEIGHT: u16 = 30;

pub fn app_with(mem: &Arc<Mem>, path: &str, content: &str) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new(content), Some(PathBuf::from(path)), vfs, None);
    app.set_root(PathBuf::from("/root"));
    app.frame_width = WIDTH;
    app.frame_height = HEIGHT;
    app.sync_view();
    app
}

pub fn plain_app(content: &str, width: u16, height: u16) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.frame_width = width;
    app.frame_height = height;
    app.sync_view();
    app
}

pub fn place_cursor(app: &mut App, offset: usize) {
    app.active_doc_mut().cursors = CursorSet::new(offset);
}

pub fn modded(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

pub fn ctrl(code: KeyCode) -> KeyInput {
    modded(
        code,
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    )
}

pub fn plain(code: KeyCode) -> KeyInput {
    modded(code, Mods::NONE)
}

pub fn sup_enter() -> KeyInput {
    modded(
        KeyCode::Enter,
        Mods {
            sup: true,
            ..Mods::NONE
        },
    )
}

pub fn back_key() -> KeyInput {
    ctrl(KeyCode::Char('['))
}

pub fn forward_key() -> KeyInput {
    ctrl(KeyCode::Char(']'))
}

pub fn press(app: &mut App, key: KeyInput) -> Effects {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(key), &mut effects);
    app.sync_view();
    effects
}

pub fn settle_file_opens(app: &mut App, mut effects: Effects) {
    for cmd in effects.cmds.drain(..) {
        assert_eq!(cmd.kind(), CmdKind::ReadFile);
        if let Some(msg) = cmd.run() {
            let mut inner = Effects::default();
            app::update(app, msg, &mut inner);
        }
    }
    app.sync_view();
}

pub fn press_and_open(app: &mut App, key: KeyInput) {
    let effects = press(app, key);
    settle_file_opens(app, effects);
}

pub fn editor_origin(app: &App) -> (u16, u16) {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let editor = rune_tui::layout::geometry(area, app).editor;
    (editor.x, editor.y)
}

pub fn click(app: &mut App, col: u16, row: u16) {
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

pub fn numbered_lines(count: usize) -> String {
    (0..count).map(|i| format!("line {i}\n")).collect()
}

/// Runs every reply-producing `Cmd` a message left behind — the reads the
/// Explorer, the finder and an ordinary open all wait on. Timer and
/// subprocess `Cmd`s are deliberately skipped rather than run inline.
pub fn settle(app: &mut App, mut effects: Effects) {
    for cmd in effects.cmds.drain(..) {
        if !matches!(cmd.kind(), CmdKind::ReadFile | CmdKind::ReadDir) {
            continue;
        }
        if let Some(msg) = cmd.run() {
            let mut inner = Effects::default();
            app::update(app, msg, &mut inner);
        }
    }
    app.sync_view();
}

pub fn press_and_settle(app: &mut App, key: KeyInput) {
    let effects = press(app, key);
    settle(app, effects);
}

pub fn focus_explorer(app: &mut App) {
    for _ in 0..2 {
        if app.focus() == Pane::Explorer {
            return;
        }
        press_and_settle(app, explorer_common::ctrl_b());
    }
    assert_eq!(
        app.focus(),
        Pane::Explorer,
        "^b must land focus on the Explorer"
    );
}

/// An `App` on `/root/a.md` with the Explorer focused and `/root` listed.
pub fn browsing_app(mem: &Arc<Mem>) -> App {
    let mut app = explorer_common::app_with(mem);
    app.set_root(PathBuf::from("/root"));
    app.frame_width = WIDTH;
    app.frame_height = HEIGHT;
    app.sync_view();
    focus_explorer(&mut app);
    app
}

pub fn arrow_to(app: &mut App, name: &str) {
    press_and_settle(app, plain(KeyCode::Home));
    let steps = app
        .explorer
        .entries
        .iter()
        .position(|entry| entry.name == name)
        .expect("the Explorer lists the entry");
    for _ in 0..steps {
        press_and_settle(app, plain(KeyCode::Down));
    }
    assert_eq!(app.explorer.entries[app.explorer.nav.cursor].name, name);
}

pub fn active_path(app: &App) -> Option<PathBuf> {
    app.active_doc().file_path.clone()
}
