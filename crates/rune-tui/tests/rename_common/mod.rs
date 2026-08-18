//! Shared setup helpers for the Rename "Done when" test suite, split
//! across `rename_bind.rs` (focus/typing, the end-to-end no-store rename,
//! and draft naming), `rename_refusals.rs` (the refusal paths),
//! `rename_gate.rs` (the extension gate and the field's own word-motion/
//! selection/undo editing), `rename_clipboard.rs` (copy/cut/paste in the
//! title), `rename_collision.rs` (the collision guard and both halves of
//! hazard 1), `rename_replace.rs` (the `[R]eplace` path against a real
//! in-memory `Store`), and `rename_focus.rs` (the WP2
//! focus-loss-is-the-commit-chokepoint suite) — TODO.md's 500-line budget split of
//! the original `rename.rs`, re-split by plan WP5 once the extension-gate
//! and clipboard packages grew `rename_bind.rs` past the ceiling again.
//! Each consumer pulls this in via `mod rename_common;` — integration test
//! files are separate binaries, so this is the one place all seven draw an
//! identical fixture from, rather than risking drift.
//!
//! Two layers live here. The `rune_fuzz::Session` layer is the primary
//! one: real stores, real key delivery, per-step invariant checking. The
//! older bare-`App` layer below it survives for the consumers that cannot
//! run under the session driver yet (`reading_view.rs`,
//! `bind_new_named.rs`, `save_state_machine.rs`,
//! `materialize_dead_writer_reentrancy.rs`, `materialize_fatal_two_docs.rs`,
//! `refused_hydration_detach.rs`) and for the handful of rename tests that
//! must observe `Effects` directly (OSC52 copies, timer arming) or hold a
//! stale `Cmd` the driver's single rename slot cannot.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use rune_db::{ClockFn, DbEvent, OpOutcome, Store};
use rune_fuzz::Session;
use rune_tui::app::{self, App};
use rune_tui::db::{Db, DbBridge, DocDb, PublishMode};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, Vfs};

// ── Session-driven fixtures ─────────────────────────────────────────────

/// The seeded document every session fixture opens.
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
        active_path(session.app()).as_deref(),
        Some(Path::new(DOC_PATH))
    );
    assert_eq!(session.app().active_doc().buffer.content(), before_content);

    assert!(session.deliver().is_none());
    assert!(session.deliver_db_all().is_none());
    assert_eq!(session.app().rename, RenameState::Idle);
    assert_eq!(
        active_path(session.app()).as_deref(),
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

// ── Bare-`App` fixtures (pre-Session consumers, see the module doc) ─────

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

/// How long [`wait_for`] gives the writer thread to post a matching event
/// before failing the test outright — long enough for the real writer
/// thread under load, short enough that a stuck wait fails fast instead of
/// hanging the whole suite.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// The bounded counterpart to [`next_event`]: once a test starts waiting
/// for a SPECIFIC outcome rather than "whatever comes next" (typing
/// enqueues its own `AppendEdit` acks ahead of the reply a test actually
/// wants), `wait_for_bootstrap_event`'s predicate already skips past those
/// — this only adds the missing timeout, so a predicate that never matches
/// fails the test with a clear message instead of blocking it forever. The
/// blocking wait itself runs on a helper thread so a non-matching `pred`
/// leaves that thread parked rather than this one.
fn wait_for(
    bridge: &Arc<DbBridge>,
    what: &'static str,
    pred: impl FnMut(&DbEvent) -> bool + Send + 'static,
) -> DbEvent {
    let bridge = Arc::clone(bridge);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(bridge.wait_for_bootstrap_event(pred));
    });
    rx.recv_timeout(EVENT_TIMEOUT)
        .unwrap_or_else(|_| panic!("timed out after {EVENT_TIMEOUT:?} waiting for {what}"))
}

/// Waits for the `MaterializePrepare` ack — the CAS-decision reply that
/// spawns the caller-side `vfs` `Cmd` (the `Save` `Cmd` the materialize
/// dance's first hop always produces).
pub fn wait_for_materialize_prep(bridge: &Arc<DbBridge>) -> DbEvent {
    wait_for(bridge, "a MaterializePrepare ack", |evt| {
        matches!(
            evt,
            DbEvent::Ok {
                result: OpOutcome::MaterializePrep(_),
                ..
            }
        )
    })
}

/// Waits for the `MaterializeRecord` ack that commits (or refuses) a save.
pub fn wait_for_materialize_record(bridge: &Arc<DbBridge>) -> DbEvent {
    wait_for(bridge, "a MaterializeRecord ack", |evt| {
        matches!(
            evt,
            DbEvent::Ok {
                result: OpOutcome::Materialize(_),
                ..
            }
        )
    })
}

/// Waits for a `Load` ack — the lost-create-race route's hand-off to an
/// ordinary load once a create-only publish loses the race.
pub fn wait_for_load(bridge: &Arc<DbBridge>) -> DbEvent {
    wait_for(bridge, "a Load ack", |evt| {
        matches!(
            evt,
            DbEvent::Ok {
                result: OpOutcome::Load(_),
                ..
            }
        )
    })
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
    app.active_doc_mut().set_doc_db_for_test(DocDb::new(
        load.doc_id.0,
        PublishMode::OverwriteExisting,
        rune_db::Seq(0),
    ));
    app.install_or_join_file_binding(load.doc_id.0, load.saved_obs);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    (app, bridge)
}

/// A store-bound pathless draft on a bare `App`: minted through the real
/// `workspace::new_untitled_document` flow with its `CreateScratch` ack
/// delivered through the ordinary `Msg::Db` path — never
/// `set_doc_db_for_test`. Bare-`App` rather than a `Session` because the
/// Enter that names a store-bound draft starts a materialize on a plain
/// key, which the fuzz driver's `SAVE-INFLIGHT-SM` checker (rightly, for
/// the flows it generates) rejects.
pub fn draft_app_with_store(mem: &Arc<Mem>) -> (App, Arc<DbBridge>) {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = DbBridge::bootstrap();
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("store");

    let mut app = App::new(
        Buffer::new(""),
        None,
        vfs,
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    let id = workspace::new_untitled_document(&mut app);
    let evt = next_event(&bridge);
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Db(evt), &mut effects);
    assert!(
        app.doc(id)
            .is_some_and(rune_tui::document::Document::is_store_bound),
        "the CreateScratch ack must bind the draft"
    );

    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    type_text(&mut app, "draft body");
    (app, bridge)
}

/// The body [`unsaved_named_app_with_store`] types into its document — the
/// bytes the loader would have found on disk had the launch actually
/// published anything, i.e. none: the fixture starts the buffer EMPTY
/// (`loader::load_buffer`'s own "a nonexistent path opens an empty
/// buffer") and types this in through the public key path afterward, so
/// the document is dirty because the user typed, not because a field was
/// poked.
pub const UNPUBLISHED_BODY: &str = "unpublished body";

/// A path-set, create-only, store-bound app whose file is ABSENT from
/// `mem` — work package A's fixture: a document that already knows its
/// name (unlike [`draft_app_with_store`]'s pathless shape) but has never
/// been published, the state a named launch onto a not-yet-existing path
/// leaves behind. Binds to a genuine scratch row (`path=''`, `CreateScratch`)
/// rather than borrowing a seeded file's real `documents` row — the same
/// row shape `handle_materialize_ack`'s own comment cites as having no CAS
/// baseline to raise `[M]`/`[D]` against, which is exactly the shape this
/// fixture needs to exercise the lost-create-race hand-off honestly.
///
/// The buffer starts empty, exactly like `loader::load_buffer`'s own
/// nonexistent-path case, then types [`UNPUBLISHED_BODY`] through the
/// ordinary key path — a document seeded non-empty here would come out of
/// `Document::new` already clean (`saved_content` is captured from the
/// INITIAL buffer), which would make every ⌘S downstream a silent no-op.
pub fn unsaved_named_app_with_store(mem: &Arc<Mem>) -> (App, Arc<DbBridge>) {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = DbBridge::bootstrap();
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("store");

    store.create_scratch().expect("enqueue create_scratch");
    let row_id = match next_event(&bridge) {
        DbEvent::Ok {
            result: OpOutcome::ScratchDocId(doc_id),
            ..
        } => doc_id.0,
        other => panic!("expected a ScratchDocId ack, got {other:?}"),
    };

    let mut app = App::new(
        Buffer::new(""),
        Some(PathBuf::from("/root/nope.md")),
        vfs,
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    app.active_doc_mut().set_doc_db_for_test(DocDb::new(
        row_id,
        PublishMode::CreateOnly,
        rune_db::Seq(0),
    ));
    app.install_or_join_file_binding(row_id, None);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    type_text(&mut app, UNPUBLISHED_BODY);
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

/// A ⌘-chorded character — copy/cut/paste's own modifier
/// (`keymap::editor_bindings::clipboard::SUP`).
pub fn sup(c: char) -> Msg {
    key(
        KeyCode::Char(c),
        Mods {
            sup: true,
            ..Mods::NONE
        },
    )
}

pub fn send(app: &mut App, msg: Msg) -> Effects {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
    effects
}

/// Types `text` into whatever pane is currently focused, one key at a
/// time — the title field for `rename_to`/`type_new_name`'s callers, or the
/// editor buffer for `unsaved_named_app_with_store`'s own call (no `^r`
/// precedes it, so focus is still the Editor).
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
