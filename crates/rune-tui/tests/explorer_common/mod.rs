//! Shared setup helpers for the WP4 Explorer "Done when" test suite, split
//! across `explorer_nav.rs` (cursor movement, opening files/directories,
//! and the parent row) and `explorer_reload.rs` (the `resolve` fallback,
//! refresh/stale-reply handling, `open_path` reactivation, and the lazy
//! `ensure_loaded` load) — TODO.md's 500-line budget split of the original
//! `explorer.rs`. Both consumers pull this in via `mod explorer_common;`
//! — integration test files are separate binaries, so this is the one
//! place both draw an identical `App`/`Mem` fixture from, rather than
//! risking the two drifting apart.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs};

/// Seeds a `Mem` vfs with `/root/a.md`, `/root/b.md`, and `/root/sub/c.md`
/// — two files plus a nested directory, so `Vfs::read_dir("/root")` lists
/// one dir ("sub") and two files ("a.md", "b.md").
pub fn seeded_vfs() -> Arc<Mem> {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/a.md"), b"a content")
        .expect("seed a.md");
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    mem.save_atomic(Path::new("/root/sub/c.md"), b"c content")
        .expect("seed sub/c.md");
    mem
}

/// An `App` whose active document is `/root/a.md` — so the Explorer's
/// `initial_root` (the active document's own directory) resolves to
/// `/root`.
pub fn app_with(mem: &Arc<Mem>) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new("a content"),
        Some(PathBuf::from("/root/a.md")),
        vfs,
        None,
    );
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();
    app
}

pub fn ctrl_b() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('b'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    }
}

pub fn key(code: KeyCode) -> KeyInput {
    KeyInput {
        code,
        mods: Mods::NONE,
    }
}

/// `^b` through the real `update`, then runs the one `ReadDir` `Cmd` it
/// enqueues and delivers its `Msg::DirLoaded` reply — the same two-step
/// production actually performs across the `Cmd` thread boundary, just
/// synchronously here.
pub fn load_explorer(app: &mut App) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(ctrl_b()), &mut effects);
    assert_eq!(effects.cmds.len(), 1, "^b must enqueue exactly one Cmd");
    assert_eq!(effects.cmds[0].kind(), CmdKind::ReadDir);
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut effects2 = Effects::default();
    app::update(app, msg, &mut effects2);
}
