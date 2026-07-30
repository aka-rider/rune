//! Shared setup helpers for the Open Tabs test suite, split across
//! `opentabs.rs` (Tabs-pane-local rendering/switching, and the close
//! guard's three resolutions) and `opentabs_global.rs` (the GLOBAL `^w`/
//! `^1`-`^0` bindings, TODO.md's §1.6 split). Each file pulls this in via
//! `mod opentabs_common;` — integration test files are separate binaries,
//! so this is the one place both draw an identical `App`/`Mem` fixture
//! from, rather than risking the two drifting apart.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

pub const WIDTH: u16 = 80;
pub const HEIGHT: u16 = 24;

pub fn seeded_vfs() -> Arc<Mem> {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/a.md"), b"a content")
        .expect("seed a.md");
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    mem
}

/// An `App` with `/root/a.md` as the initial (sole) document, no store
/// bound (`db: None`) — so any save on any document funnels through the
/// no-store `Msg::SaveDone` fallback (Assumption A1), matching an
/// Explorer-opened document's own shape.
pub fn app_with(mem: &Arc<Mem>) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new("a content"),
        Some(PathBuf::from("/root/a.md")),
        vfs,
        None,
    );
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

/// Opens `/root/b.md` as a second document via the real `workspace::
/// open_path` — mirroring how a real session accumulates tabs.
pub fn open_second(app: &mut App) -> DocumentId {
    let first = app.active;
    workspace::open_path(app, Path::new("/root/b.md"));
    let second = app.active;
    assert_ne!(
        first, second,
        "test setup: b.md must open as a NEW document"
    );
    second
}

pub fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}
