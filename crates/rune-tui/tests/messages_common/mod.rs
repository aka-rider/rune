//! Shared setup helpers for the message-log pane's test suite, split across
//! `messages.rs` (state/keyboard/timer coverage) and `messages_mouse.rs`
//! (drag/click/copy coverage), following the `merge_common` pattern every
//! other split-out test suite in this directory already uses. Each
//! consumer pulls this in via `mod messages_common;`.
#![allow(dead_code)]

use rune_fuzz::Session;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;

pub const WIDTH: u16 = 80;
pub const HEIGHT: u16 = 24;

/// A `Session` whose sole document holds `content` — the message-log pane
/// tests never care which path it lives at, only what's typed into it.
pub fn app_for(content: &str) -> Session {
    let mut session = Session::open("/draft.md", content);

    // `Session::boot` mints its own untitled draft before opening the
    // seeded document, so `documents.len()` starts at 2 — close that
    // surplus draft through the real close path so a test sees the same
    // single-document starting point the pre-migration `App::new` fixture
    // did.
    let seed = session.app().active;
    let draft = *session
        .app()
        .documents
        .order()
        .iter()
        .find(|&&id| id != seed)
        .expect("Session::boot mints a draft alongside the seeded document");
    workspace::request_close(session.app_mut(), draft, &mut Effects::default());

    session
}

pub fn frame_text(session: &mut Session) -> String {
    session.grid(WIDTH, HEIGHT).concat()
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
