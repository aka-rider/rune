use std::path::Path;
use std::sync::Arc;

use rune_db::{ClockFn, Store};
use rune_fuzz::Session;
use rune_tui::db::{Db, DbBridge};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::workspace;

use rune_vfs::{Mem, Vfs, VfsTestExt};

use super::seeded_vfs;

pub const DOC_PATH: &str = "/root/a.md";
pub const DOC_CONTENT: &str = "a content";

pub fn key_input(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

pub fn plain_key(code: KeyCode) -> KeyInput {
    key_input(code, Mods::NONE)
}

pub fn ctrl_key(c: char) -> KeyInput {
    key_input(
        KeyCode::Char(c),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    )
}

/// A ⌘-chorded character — save's and the clipboard commands' own modifier.
pub fn sup_key(c: char) -> KeyInput {
    key_input(
        KeyCode::Char(c),
        Mods {
            sup: true,
            ..Mods::NONE
        },
    )
}

/// A `Session` over `mem` with a fresh in-memory `Store`, opened on `path`
/// — the session's boot drains the `Load` ack, so the seeded document is
/// store-bound through the real ack path, never `set_doc_db_for_test`.
pub fn store_session(mem: &Arc<Mem>, path: &str) -> Session {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = DbBridge::bootstrap();
    let store = Store::open_in_memory(clock, vfs, bridge.on_event()).expect("store");
    Session::open_with_db(path, Arc::clone(mem), Db::new(store, bridge, false))
}

/// The store-bound shape: `/root/a.md` open, active, and bound to a real
/// recovery row — a rename here routes through the store's own rename op,
/// drained with `deliver_db`.
pub fn bound_session() -> (Session, Arc<Mem>) {
    let mem = seeded_vfs();
    let session = store_session(&mem, DOC_PATH);
    (session, mem)
}

/// The `Cmd`-route shape: `/root/a.md` opened through `workspace::open_path`
/// with its `Load` ack deliberately NOT delivered — exactly an
/// Explorer-opened document's own state before the ack lands. An unbound
/// document's rename takes the no-store `rename_excl` `Cmd`, discharged
/// with `Session::deliver`.
pub fn unbound_session() -> (Session, Arc<Mem>) {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/seed.md"), b"seed")
        .expect("seed the session's own bootstrap document");
    let mut session = store_session(&mem, "/root/seed.md");
    workspace::open_path(session.app_mut(), Path::new(DOC_PATH)).expect("open a.md");
    session.app_mut().active_doc_mut().focused = true;
    (session, mem)
}

/// A pathless draft minted through the real `workspace::new_untitled_document`
/// flow, active and focused, its `CreateScratch` ack NOT delivered — so a
/// name commit takes the no-store exclusive-create `Cmd` route.
pub fn draft_session() -> (Session, Arc<Mem>) {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/seed.md"), b"seed")
        .expect("seed the session's own bootstrap document");
    let mut session = store_session(&mem, "/root/seed.md");
    workspace::new_untitled_document(session.app_mut());
    session.app_mut().active_doc_mut().focused = true;
    (session, mem)
}

/// [`draft_session`], with the `CreateScratch` ack delivered — the draft is
/// store-bound (create-only), so a name commit routes through
/// `save::bind_new_now`'s materialize dance.
pub fn bound_draft_session() -> (Session, Arc<Mem>) {
    let (mut session, mem) = draft_session();
    assert!(session.deliver_db_all().is_none());
    assert!(
        session.app().active_doc().is_store_bound(),
        "the CreateScratch ack must bind the draft"
    );
    (session, mem)
}

/// `^r`, asserting the title actually took focus.
pub fn open_title(session: &mut Session) {
    assert!(session.key(ctrl_key('r')).is_none());
    assert_eq!(session.app().focus(), Pane::Title);
}

/// `^r` then clear the STEM (the extension is fenced off by the gate) and
/// type `name` — WITHOUT pressing Enter, so the caller can drive a
/// different blur gesture against the still-uncommitted name.
pub fn set_name(session: &mut Session, name: &str) {
    open_title(session);
    assert!(session.key(ctrl_key('a')).is_none());
    assert!(session.key(plain_key(KeyCode::Backspace)).is_none());
    assert!(session.type_(name).is_none());
}

/// [`set_name`], committed with Enter.
pub fn commit_name(session: &mut Session, name: &str) {
    set_name(session, name);
    assert!(session.key(plain_key(KeyCode::Enter)).is_none());
}

/// A draft's title seeds as a bare `.md` with the gate unlocked, so there
/// is no stem to clear: `^r`, type `name` in front of the extension, Enter.
pub fn name_draft(session: &mut Session, name: &str) {
    open_title(session);
    assert!(session.type_(name).is_none());
    assert!(session.key(plain_key(KeyCode::Enter)).is_none());
}

/// Every refusal leaves the machine `Idle`, `file_path` unchanged, and the
/// buffer byte-identical — and draining every deferred `Cmd` and store op
/// afterwards must leave the seeded file exactly as published, proving no
/// rename was enqueued anywhere.
pub fn assert_refused(session: &mut Session, mem: &Arc<Mem>, before_content: &str) {
    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(
        super::active_path(session.app()).as_deref(),
        Some(Path::new(DOC_PATH))
    );
    assert_eq!(session.app().active_doc().buffer.content(), before_content);

    assert!(session.deliver().is_none());
    assert!(session.deliver_db_all().is_none());
    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(
        super::active_path(session.app()).as_deref(),
        Some(Path::new(DOC_PATH))
    );
    assert_eq!(
        mem.read(Path::new(DOC_PATH)).expect("a.md still readable"),
        DOC_CONTENT.as_bytes(),
        "a refused rename must leave the file exactly as published"
    );
}

/// Seeds `/root/b.md` and commits a rename onto it, leaving the collision
/// reply UNDELIVERED (`Session::deliver` is the caller's) — so a test can
/// interleave its own messages before the reply lands.
pub fn collide_pending(session: &mut Session, mem: &Arc<Mem>) {
    mem.save_atomic(Path::new("/root/b.md"), b"theirs")
        .expect("seed b.md");
    commit_name(session, "b");
    assert!(
        matches!(session.app().rename, RenameState::Committing { .. }),
        "the commit must be in flight before the collision reply lands"
    );
}

/// [`collide_pending`], with the collision reply delivered.
pub fn collide(session: &mut Session, mem: &Arc<Mem>) {
    collide_pending(session, mem);
    assert!(session.deliver().is_none());
}
