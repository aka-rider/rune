//! Shared setup helpers for the Rename "Done when" test suite, split
//! across `rename_bind.rs` (focus/typing, the refusals, the end-to-end
//! no-store rename, and draft naming), `rename_collision.rs` (the
//! collision guard and both halves of hazard 1), `rename_replace.rs`
//! (the `[R]eplace` path against a real in-memory `Store`), and
//! `rename_focus.rs` (the WP2 focus-loss-is-the-commit-chokepoint suite)
//! — TODO.md's §1.6 split of the original `rename.rs`. Each consumer pulls
//! this in via `mod rename_common;` — integration test files are separate
//! binaries, so this is the one place all four draw an identical
//! `App`/`Mem` fixture from, rather than risking drift.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_db::{ClockFn, DbEvent, OpOutcome, Store};
use rune_tui::app::{self, App};
use rune_tui::db::{Db, DbBridge, DocDb};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, Vfs};

pub const WIDTH: u16 = 80;
pub const HEIGHT: u16 = 24;

pub fn seeded_vfs() -> Arc<Mem> {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/a.md"), b"a content")
        .expect("seed a.md");
    mem
}

/// An `App` on `/root/a.md` with NO store bound — the no-store `Cmd` route
/// (and an Explorer-opened document's own shape).
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

/// Blocks for whatever `DbEvent` the writer thread delivers next, buffered
/// on `bridge`'s own `Bootstrap` sink: nothing calls `DbBridge::attach` in
/// these tests, so every ack — the seed `Load` here, a later rename/replace/
/// materialize ack at the call sites below — lands there instead. A genuine
/// rendezvous with the writer, not a paced wait; each call site is only
/// ever waiting on the one op it just enqueued, so "the next event" is
/// unambiguous.
pub fn next_event(bridge: &DbBridge) -> DbEvent {
    bridge.wait_for_bootstrap_event(|_| true)
}

/// The same `App`, but bound to a REAL in-memory `Store` sharing `mem` as
/// its filesystem — so the store's own rename ops act on the very files
/// these tests seeded and assert on.
///
/// The returned bridge is left in its `Bootstrap` sink (never `attach`ed),
/// so every later `DbEvent` the writer thread posts stays buffered there
/// for `next_event` to drain.
pub fn app_with_store(mem: &Arc<Mem>) -> (App, Arc<DbBridge>) {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = DbBridge::bootstrap();
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("store");

    // Seed a real `documents` row for the bootstrap document through the
    // ordinary `Load` op, so the store's rename ops have something to
    // rebind — no hand-written SQL from outside the crate.
    store.load(Path::new("/root/a.md")).expect("enqueue load");
    let load = match next_event(&bridge) {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => *load,
        other => panic!("expected a Load ack, got {other:?}"),
    };

    let mut app = App::new(
        Buffer::new("a content"),
        Some(PathBuf::from("/root/a.md")),
        vfs,
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    app.active_doc_mut().db = Some(DocDb::new(
        load.doc_id,
        load.saved_obs.unwrap_or(0),
        false,
        0,
    ));
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    (app, bridge)
}

/// A pathless draft bound to a real `Store` — not the CLI's own shape today
/// (the default untitled document opens with `db: None`, see
/// `crates/rune-tui/TODO.md`, "no recovery journal for the default
/// untitled document"), but the routing this exercises —
/// `rename::bind_new`'s store branch and `materialize_ack::handle_materialize_ack`'s
/// bind — does not care how the store binding was acquired, only that one
/// exists.
pub fn draft_app_with_store(mem: &Arc<Mem>) -> (App, Arc<DbBridge>) {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = DbBridge::bootstrap();
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("store");

    // Mints a real `doc_id` the same way `app_with_store` does — a fresh
    // draft has no `documents` row of its own to load, so this borrows one
    // via an ordinary `Load` against an unrelated seeded file.
    store
        .load(Path::new("/root/seed.md"))
        .expect("enqueue load");
    let load = match next_event(&bridge) {
        DbEvent::Ok {
            result: OpOutcome::Load(load),
            ..
        } => *load,
        other => panic!("expected a Load ack, got {other:?}"),
    };

    let mut app = App::new(
        Buffer::new("draft body"),
        None,
        vfs,
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    // The default untitled document's own shape (`App::new_untitled`).
    app.active_doc_mut().display_name = Some("Untitled 1".to_string());
    app.active_doc_mut().db = Some(DocDb::new(
        load.doc_id,
        load.saved_obs.unwrap_or(0),
        true,
        0,
    ));
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    (app, bridge)
}

pub fn key(code: KeyCode, mods: Mods) -> Msg {
    Msg::Key(KeyInput { code, mods })
}

pub fn plain(code: KeyCode) -> Msg {
    key(code, Mods::NONE)
}

pub fn ctrl(c: char) -> Msg {
    key(
        KeyCode::Char(c),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    )
}

pub fn send(app: &mut App, msg: Msg) -> Effects {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
    effects
}

/// Types `text` into the focused title field, one key at a time.
pub fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        send(app, plain(KeyCode::Char(ch)));
    }
}

/// `^r` then select-all-equivalent: clear the STEM (the extension is fenced
/// off by the gate — see `title.rs`'s `TitleField::window` — so a plain
/// backspace loop to an empty overall TEXT would spin forever once the
/// stem itself is gone), then type `name`.
pub fn rename_to(app: &mut App, name: &str) -> Effects {
    send(app, ctrl('r'));
    assert_eq!(app.focus(), Pane::Title);
    send(app, ctrl('a'));
    send(app, plain(KeyCode::Backspace));
    type_text(app, name);
    send(app, plain(KeyCode::Enter))
}

/// `^r`, clear the stem, then type `name` — WITHOUT pressing Enter, so the
/// caller can drive a DIFFERENT blur gesture and observe what it does with
/// the still-uncommitted name.
pub fn type_new_name(app: &mut App, name: &str) {
    send(app, ctrl('r'));
    assert_eq!(app.focus(), Pane::Title);
    send(app, ctrl('a'));
    send(app, plain(KeyCode::Backspace));
    type_text(app, name);
}

pub fn active_path(app: &App) -> Option<PathBuf> {
    app.active_doc().file_path.clone()
}

/// Every refusal leaves the machine `Idle`, `file_path` unchanged, the
/// buffer byte-identical, and no `Cmd` enqueued.
pub fn assert_refused(app: &App, effects: &Effects, before_content: &str) {
    assert_eq!(app.rename, rune_tui::rename::RenameState::Idle);
    assert_eq!(active_path(app).as_deref(), Some(Path::new("/root/a.md")));
    assert_eq!(app.active_doc().buffer.content(), before_content);
    assert!(
        !effects
            .cmds
            .iter()
            .any(|c| c.kind() == rune_tui::runtime::CmdKind::Rename),
        "a refused rename must enqueue no Rename Cmd"
    );
}

/// Drives a rename into a collision and returns the reply message.
pub fn collide(app: &mut App, mem: &Arc<Mem>) -> Msg {
    mem.save_atomic(Path::new("/root/b.md"), b"theirs")
        .expect("seed b.md");
    let mut effects = rename_to(app, "b");
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == rune_tui::runtime::CmdKind::Rename)
        .expect("a Rename Cmd");
    cmd.run().expect("a reply")
}
