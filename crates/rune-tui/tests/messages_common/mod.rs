//! Shared setup helpers for the message-log pane's test suite, split across
//! `messages.rs` (state/keyboard/timer coverage) and `messages_mouse.rs`
//! (drag/click/copy coverage), following the `merge_common` pattern every
//! other split-out test suite in this directory already uses. Each
//! consumer pulls this in via `mod messages_common;`.
#![allow(dead_code)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::Msg;
use rune_tui::testgrid;
use rune_vfs::Mem;

pub const WIDTH: u16 = 80;
pub const HEIGHT: u16 = 24;

pub fn app_for(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.frame_width = WIDTH;
    app.frame_height = HEIGHT;
    app.sync_view();
    app
}

pub fn frame_text(app: &App) -> String {
    testgrid::grid(app, WIDTH, HEIGHT).concat()
}

pub fn key(code: KeyCode) -> Msg {
    Msg::Key(KeyInput {
        code,
        mods: Mods::NONE,
    })
}

pub fn ctrl_e() -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char('e'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

/// `⌘C` — one of the pane's own two copy chords, the exact chord the
/// editor's own `Copy` row binds too.
pub fn super_c() -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char('c'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    })
}
