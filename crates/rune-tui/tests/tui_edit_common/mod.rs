#![allow(dead_code)]

use std::sync::Arc;

use ratatui::buffer::Buffer as RtBuffer;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

pub const WIDTH: u16 = 80;
pub const HEIGHT: u16 = 24;

pub fn app_for(content: &str, cursor_offset: usize) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().focused = true;
    app.active_doc_mut()
        .set_cursors(CursorSet::new(cursor_offset.min(content.len())));
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

pub fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

pub fn press(app: &mut App, code: KeyCode, mods: Mods) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(key(code, mods)), &mut effects);
    app.sync_view();
}

pub const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};
pub const ALT: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: false,
};
pub const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};
pub const SUP_SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: true,
};

pub fn render_to_test_backend(app: &App) -> RtBuffer {
    testgrid::draw(app, WIDTH, HEIGHT)
}

pub fn full_text(buf: &RtBuffer) -> String {
    let mut s = String::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if let Some(cell) = buf.cell((x, y)) {
                s.push_str(cell.symbol());
            }
        }
        s.push('\n');
    }
    s
}

pub fn app_at_path(content: &str, path: &str, cursor_offset: usize) -> App {
    let vfs = Arc::new(Mem::new());
    let launch =
        rune_tui::resolved::ResolvedPath::resolve(vfs.as_ref(), &std::path::PathBuf::from(path))
            .expect("the launch path resolves");
    let mut app = App::new(Buffer::new(content), Some(launch), vfs, None);
    app.active_doc_mut().focused = true;
    app.active_doc_mut()
        .set_cursors(CursorSet::new(cursor_offset.min(content.len())));
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

pub fn split_carets(marked: &str) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut carets = Vec::new();
    for ch in marked.chars() {
        if ch == '|' {
            carets.push(text.len());
        } else {
            text.push(ch);
        }
    }
    (text, carets)
}

pub fn with_carets(marked: &str) -> App {
    let (text, carets) = split_carets(marked);
    let mut app = app_for(&text, 0);
    app.active_doc_mut()
        .set_cursors(CursorSet::new_from_positions(&carets));
    app.sync_view();
    app
}

pub fn with_carets_at(path: &str, marked: &str) -> App {
    let (text, carets) = split_carets(marked);
    let mut app = app_at_path(&text, path, 0);
    app.active_doc_mut()
        .set_cursors(CursorSet::new_from_positions(&carets));
    app.sync_view();
    app
}

pub fn marked(app: &mut App) -> String {
    let doc = app.active_doc_mut();
    let mut text = doc.buffer.content().to_string();
    let mut carets: Vec<usize> = doc
        .cursors()
        .all()
        .iter()
        .map(|c| c.position.get())
        .collect();
    carets.sort_unstable_by(|a, b| b.cmp(a));
    for at in carets {
        text.insert(at, '|');
    }
    text
}

pub fn enter(app: &mut App) {
    press(app, KeyCode::Enter, Mods::NONE);
}

pub fn backspace(app: &mut App) {
    press(app, KeyCode::Backspace, Mods::NONE);
}

pub fn undo(app: &mut App) {
    press(app, KeyCode::Char('z'), SUP);
}
