use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use rune_db::{ClockFn, DbEvent, OpOutcome, Store};
use rune_tui::app::{self, App};
use rune_tui::db::{Db, DbBridge, DocDb, PublishMode};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, Vfs, VfsTestExt};

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
