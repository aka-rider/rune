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
    app.active_doc_mut().cursors = CursorSet::new(cursor_offset.min(content.len()));
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
