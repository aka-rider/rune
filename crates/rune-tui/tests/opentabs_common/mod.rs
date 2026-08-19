//! Shared setup helpers for the Open Tabs test suite, split across
//! `opentabs.rs` (Tabs-pane-local rendering/switching, and the close
//! guard's three resolutions) and `opentabs_global.rs` (the GLOBAL `^w`/
//! `^1`-`^0` bindings, TODO.md's 500-line budget). Each file pulls this in via
//! `mod opentabs_common;` — integration test files are separate binaries,
//! so this is the one place both draw an identical `Session` fixture from,
//! rather than risking the two drifting apart.
#![allow(dead_code)]

use std::path::Path;

use rune_fuzz::Session;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::Effects;
use rune_tui::workspace;
use rune_vfs::VfsTestExt;

pub const WIDTH: u16 = 80;
pub const HEIGHT: u16 = 24;

/// A `Session` seeded with `/root/a.md` as the initial (sole) document and
/// `/root/b.md` sitting alongside it on disk, ready for `open_second`.
pub fn open_seeded() -> Session {
    let mut session = Session::open("/root/a.md", "a content");
    session
        .app()
        .vfs
        .save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");

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

/// Opens `/root/b.md` as a second document via the real `workspace::
/// open_path` — mirroring how a real session accumulates tabs.
pub fn open_second(session: &mut Session) -> DocumentId {
    let first = session.app().active;
    workspace::open_path(session.app_mut(), Path::new("/root/b.md"));
    let second = session.app().active;
    assert_ne!(
        first, second,
        "test setup: b.md must open as a NEW document"
    );
    second
}

pub fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

/// Renders `session`'s current frame and concatenates every row — the
/// whole-frame text search idiom the Open Tabs render assertions use.
pub fn frame_text(session: &mut Session) -> String {
    session.grid(WIDTH, HEIGHT).concat()
}
