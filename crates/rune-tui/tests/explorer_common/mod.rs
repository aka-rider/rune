//! Shared setup helpers for the Explorer test suite, split
//! across `explorer_nav.rs` (cursor movement, opening files/directories,
//! and the parent row) and `explorer_reload.rs` (the `resolve` fallback,
//! refresh/stale-reply handling, `open_path` reactivation, and the lazy
//! `ensure_loaded` load) — this is the 500-line-budget split of the original
//! `explorer.rs`. Both consumers, plus `trash.rs`, pull this in via `mod
//! explorer_common;` — integration test files are separate binaries, so
//! this is the one place all three draw an identical fixture from, rather
//! than risking them drifting apart.
//!
//! `app_with`/`load_explorer` stay `App`-shaped: `navhistory_common`'s
//! `browsing_app` (embedded here via `#[path]`, shared with
//! `navhistory_browse.rs`) still builds on them, and `Session` has no way
//! to hand a caller its `App` back by value — only `&`/`&mut` through
//! `app()`/`app_mut()` — so a `Session`-returning fixture can never stand
//! in for them without rewriting that whole chain too. `open_seeded`/
//! `drive_load_explorer` are the `Session`-shaped counterparts driving
//! `explorer_nav.rs`, `explorer_reload.rs`, and `trash.rs`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rune_core::buffer::Buffer;
use rune_db::{ClockFn, Store};
use rune_fuzz::Session;
use rune_tui::app::{self, App};
use rune_tui::db::{Db, DbBridge};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs, VfsTestExt};

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
/// `/root`. Kept `App`-shaped for `navhistory_common::browsing_app`, its
/// last remaining consumer.
pub fn app_with(mem: &Arc<Mem>) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new("a content"),
        Some(
            rune_tui::resolved::ResolvedPath::resolve(
                vfs.as_ref(),
                std::path::Path::new(&PathBuf::from("/root/a.md")),
            )
            .expect("the launch path resolves"),
        ),
        vfs,
        None,
    );
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();
    app
}

/// [`app_with`]'s `Session`-backed counterpart: same `/root/a.md` +
/// `b.md` + `sub/c.md` fixture, wired to a real in-memory `Store` and
/// driven through `rune_fuzz::Session`'s checked-invariant actions.
pub fn open_seeded(mem: &Arc<Mem>) -> Session {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let bridge = DbBridge::bootstrap();
    let clock: ClockFn = Arc::new(|| SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000));
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event())
        .expect("open in-memory store");
    let db = Db::new(store, Arc::clone(&bridge), false);
    let mut session = Session::open_with_db("/root/a.md", Arc::clone(mem), db);

    // `Session::boot` mints its own untitled draft before opening the
    // seeded document, so `documents.len()` starts at 2, not 1 like the
    // plain `App::new` fixture `app_with` builds — close that surplus
    // draft through the real close path so a `Session`-backed test sees
    // the same single-document starting point.
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

/// [`load_explorer`], driven over a [`Session`]'s own `App` — the real
/// `^b` + `ReadDir` `Cmd` round trip, reached through `app_mut()` since
/// `Session`'s own checked actions never surface the enqueued `Cmd` for a
/// test to run by hand.
pub fn drive_load_explorer(session: &mut Session) {
    load_explorer(session.app_mut());
}
