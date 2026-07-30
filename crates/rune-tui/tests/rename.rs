//! Rename "Done when" tests: the refusals, the `^r`/title-field gesture,
//! the end-to-end no-store rename, the collision guard and both halves of
//! hazard 1 (a prompt that is never raised, and one that is displaced
//! later), the `[R]eplace` path against a real in-memory `Store`, the
//! stale-ticket drop, and draft naming.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_db::{ClockFn, DbEvent, OpOutcome, Store};
use rune_tui::app::{self, App};
use rune_tui::banner::{self, GuardKind, Modal};
use rune_tui::db::{Db, DbBridge, DocDb};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::rename::RenameState;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::{footer, workspace};

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, Vfs};

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn seeded_vfs() -> Arc<Mem> {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/a.md"), b"a content")
        .expect("seed a.md");
    mem
}

/// An `App` on `/root/a.md` with NO store bound — the no-store `Cmd` route
/// (and an Explorer-opened document's own shape).
fn app_with(mem: &Arc<Mem>) -> App {
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
fn next_event(bridge: &DbBridge) -> DbEvent {
    bridge.wait_for_bootstrap_event(|_| true)
}

/// The same `App`, but bound to a REAL in-memory `Store` sharing `mem` as
/// its filesystem — so the store's own rename ops act on the very files
/// these tests seeded and assert on.
///
/// The returned bridge is left in its `Bootstrap` sink (never `attach`ed),
/// so every later `DbEvent` the writer thread posts stays buffered there
/// for `next_event` to drain.
fn app_with_store(mem: &Arc<Mem>) -> (App, Arc<DbBridge>) {
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
fn draft_app_with_store(mem: &Arc<Mem>) -> (App, Arc<DbBridge>) {
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

fn key(code: KeyCode, mods: Mods) -> Msg {
    Msg::Key(KeyInput { code, mods })
}

fn plain(code: KeyCode) -> Msg {
    key(code, Mods::NONE)
}

fn ctrl(c: char) -> Msg {
    key(
        KeyCode::Char(c),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    )
}

fn send(app: &mut App, msg: Msg) -> Effects {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
    effects
}

/// Types `text` into the focused title field, one key at a time.
fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        send(app, plain(KeyCode::Char(ch)));
    }
}

/// `^r` then select-all-equivalent: clear the field, then type `name`.
fn rename_to(app: &mut App, name: &str) -> Effects {
    send(app, ctrl('r'));
    assert_eq!(app.focus(), Pane::Title);
    while !app.title.text.is_empty() {
        send(app, plain(KeyCode::Backspace));
    }
    type_text(app, name);
    send(app, plain(KeyCode::Enter))
}

/// `^r`, clear the field, then type `name` — WITHOUT pressing Enter, so the
/// caller can drive a DIFFERENT blur gesture and observe what it does with
/// the still-uncommitted name.
fn type_new_name(app: &mut App, name: &str) {
    send(app, ctrl('r'));
    assert_eq!(app.focus(), Pane::Title);
    while !app.title.text.is_empty() {
        send(app, plain(KeyCode::Backspace));
    }
    type_text(app, name);
}

fn active_path(app: &App) -> Option<PathBuf> {
    app.active_doc().file_path.clone()
}

// ── Focus and typing ────────────────────────────────────────────────────

/// `^r` focuses the title, seeded with the file's STEM, and typing there
/// never touches the buffer — the `PANE-NO-BLEED` property, asserted
/// directly.
#[test]
fn ctrl_r_focuses_the_title_and_typing_never_touches_the_buffer() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));
    assert_eq!(app.focus(), Pane::Title);
    assert_eq!(app.title.text, "a", "seeded with the stem, not 'a.md'");

    type_text(&mut app, "xyz");
    assert_eq!(app.title.text, "axyz");
    assert_eq!(
        app.active_doc().buffer.content(),
        before,
        "a keystroke aimed at the file name must never reach the buffer"
    );
}

/// `Esc` reverts to the committed name and refocuses the editor without
/// renaming anything.
#[test]
fn escape_reverts_the_field_and_renames_nothing() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    type_text(&mut app, "zzz");
    send(&mut app, plain(KeyCode::Escape));

    assert_eq!(app.title.text, "a");
    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/a.md")));
    assert!(mem.read(Path::new("/root/a.md")).is_ok());
}

/// Up at the top of the buffer focuses the title (a contextual gesture, no
/// new binding).
#[test]
fn up_at_the_top_of_the_editor_focuses_the_title() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    send(&mut app, plain(KeyCode::Up));
    assert_eq!(app.focus(), Pane::Title);
}

// ── Refusals ────────────────────────────────────────────────────────────

/// Every refusal leaves the machine `Idle`, `file_path` unchanged, the
/// buffer byte-identical, and no `Cmd` enqueued.
fn assert_refused(app: &App, effects: &Effects, before_content: &str) {
    assert_eq!(app.rename, RenameState::Idle);
    assert_eq!(active_path(app).as_deref(), Some(Path::new("/root/a.md")));
    assert_eq!(app.active_doc().buffer.content(), before_content);
    assert!(
        !effects.cmds.iter().any(|c| c.kind() == CmdKind::Rename),
        "a refused rename must enqueue no Rename Cmd"
    );
}

/// Decision 12: a read-only document's title cannot be focused AT ALL — the
/// refusal now happens at `^r` itself (`App::focus_title`), before there is
/// ever anything to type. Focusing the Help document's title would
/// otherwise hold the user in a field describing a document they can never
/// rename; removing the illegal state beats guarding it later inside
/// `rename::begin`.
#[test]
fn a_read_only_document_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    app.active_doc_mut().read_only = true;
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));

    assert_eq!(app.focus(), Pane::Editor, "the title must never gain focus");
    assert_eq!(
        app.status_message.as_deref(),
        Some("this document is read-only")
    );
    assert_eq!(app.active_doc().buffer.content(), before);
}

/// The no-store `save_cmd` captures `path` in its closure and would
/// republish at the OLD name, so a rename mid-save is refused.
#[test]
fn a_save_in_flight_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    app.active_doc_mut().save_in_flight = true;
    let before = app.active_doc().buffer.content().to_string();

    let effects = rename_to(&mut app, "b");
    assert_refused(&app, &effects, &before);
}

#[test]
fn an_empty_name_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));
    while !app.title.text.is_empty() {
        send(&mut app, plain(KeyCode::Backspace));
    }
    let effects = send(&mut app, plain(KeyCode::Enter));
    assert_refused(&app, &effects, &before);
}

/// `/` is filtered at the keystroke, so it can never even reach the name —
/// the field's own validation is the second line of defence.
#[test]
fn a_slash_never_enters_the_field() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    type_text(&mut app, "b/c");
    assert_eq!(
        app.title.text, "abc",
        "'/' must be filtered at the keystroke"
    );
}

/// Committing an unchanged name is a plain refocus, never a rename of a
/// file onto its own path.
#[test]
fn an_unchanged_name_refuses_to_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));
    let effects = send(&mut app, plain(KeyCode::Enter));

    assert_eq!(app.focus(), Pane::Editor);
    assert_refused(&app, &effects, &before);
}

// ── End to end, no store ────────────────────────────────────────────────

/// One `CmdKind::Rename`, run it, feed the reply: the file moves, the tab
/// and title show the new name, and `is_dirty()` is UNCHANGED — a rename
/// is not a save (§1.4.2).
#[test]
fn end_to_end_no_store_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    // Make the document dirty, so "stays dirty" is actually observable.
    app.active_doc_mut().mark_dirty_from_hydration();
    let dirty_before = app.is_dirty();
    assert!(dirty_before, "test setup: the document must be dirty");

    let mut effects = rename_to(&mut app, "b");
    let cmds: Vec<_> = effects
        .cmds
        .drain(..)
        .filter(|c| c.kind() == CmdKind::Rename)
        .collect();
    assert_eq!(cmds.len(), 1, "exactly one Rename Cmd");
    assert!(matches!(app.rename, RenameState::Committing { .. }));

    let msg = cmds.into_iter().next().unwrap().run().expect("a reply");
    send(&mut app, msg);

    assert_eq!(app.rename, RenameState::Idle);
    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/b.md")));
    assert_eq!(mem.read(Path::new("/root/b.md")).unwrap(), b"a content");
    assert!(
        mem.read(Path::new("/root/a.md")).is_err(),
        "the old name must be gone"
    );
    assert_eq!(app.active_doc().file_name(), "b.md");
    assert_eq!(
        app.is_dirty(),
        dirty_before,
        "a rename must not change dirty state"
    );
}

/// A `rename_excl` I/O failure surfaces as an error modal, leaves
/// `file_path` alone, and returns the machine to `Idle`.
#[test]
fn a_rename_io_failure_raises_the_error_modal_and_changes_nothing() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    let mut effects = rename_to(&mut app, "b");
    mem.fail_next(
        rune_vfs::OpKind::RenameExcl,
        std::io::ErrorKind::PermissionDenied,
    );
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");
    send(&mut app, cmd.run().expect("a reply"));

    assert_eq!(app.rename, RenameState::Idle);
    assert!(matches!(app.modal, Some(Modal::Error(_))));
    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/a.md")));
    assert_eq!(mem.read(Path::new("/root/a.md")).unwrap(), b"a content");
}

/// A second commit while one is in flight is REFUSED, never queued.
#[test]
fn a_second_commit_while_one_is_in_flight_is_refused() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    let first = rename_to(&mut app, "b");
    assert_eq!(
        first
            .cmds
            .iter()
            .filter(|c| c.kind() == CmdKind::Rename)
            .count(),
        1
    );
    assert!(matches!(app.rename, RenameState::Committing { .. }));

    let second = rename_to(&mut app, "c");
    assert_eq!(
        second
            .cmds
            .iter()
            .filter(|c| c.kind() == CmdKind::Rename)
            .count(),
        0,
        "the second commit must enqueue nothing"
    );
}

// ── Collision + hazard 1 ────────────────────────────────────────────────

/// Drives a rename into a collision and returns the reply message.
fn collide(app: &mut App, mem: &Arc<Mem>) -> Msg {
    mem.save_atomic(Path::new("/root/b.md"), b"theirs")
        .expect("seed b.md");
    let mut effects = rename_to(app, "b");
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");
    cmd.run().expect("a reply")
}

/// A collision with no modal up raises the guard, enters `Collision`, and
/// the footer names the target.
#[test]
fn a_collision_raises_the_guard_and_the_footer_names_the_target() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);
    send(&mut app, reply);

    assert!(matches!(app.rename, RenameState::Collision { .. }));
    assert!(matches!(
        app.modal,
        Some(Modal::Guard(ref p)) if matches!(p.kind, GuardKind::RenameCollision { .. })
    ));
    let text = footer::footer_text(&app);
    assert!(
        text.contains("b.md"),
        "footer must name the target: {text:?}"
    );
    assert!(text.contains(banner::DIRTY_CLOSE_CANCEL_LABEL));

    // Both files are still intact — a collision writes nothing.
    assert_eq!(mem.read(Path::new("/root/a.md")).unwrap(), b"a content");
    assert_eq!(mem.read(Path::new("/root/b.md")).unwrap(), b"theirs");
}

/// **Hazard 1a**: an `Error` is up when the collision reply lands, so
/// `set_modal` returns false and the prompt is never raised. The machine
/// must stay `Idle` rather than wait on an invisible prompt.
#[test]
fn a_collision_suppressed_by_a_live_error_leaves_the_machine_idle() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);

    banner::report_error(&mut app, "something else went wrong");
    send(&mut app, reply);

    assert_eq!(
        app.rename,
        RenameState::Idle,
        "never wait on a prompt that was never raised"
    );
    assert!(
        matches!(app.modal, Some(Modal::Error(_))),
        "the unread error must survive"
    );
}

/// **Hazard 1b**: an `Error` raised LATER displaces a live collision guard.
/// `clear_modal`'s dismissal hook must return the machine to `Idle` and put
/// the user back in the title field with the typed name.
#[test]
fn an_error_displacing_the_guard_also_cancels_the_collision() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);
    send(&mut app, reply);
    assert!(matches!(app.rename, RenameState::Collision { .. }));

    banner::report_error(&mut app, "boom");

    assert_eq!(app.rename, RenameState::Idle);
    assert_eq!(app.focus(), Pane::Title);
    assert_eq!(app.title.text, "b", "the TYPED name must still be there");
}

/// `Esc` on the guard clears it, returns to `Idle`, and leaves the field
/// holding the typed name (not the old committed one).
#[test]
fn escape_on_the_collision_guard_returns_to_the_title_with_the_typed_name() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);
    send(&mut app, reply);

    send(&mut app, plain(KeyCode::Escape));

    assert_eq!(app.rename, RenameState::Idle);
    assert!(app.modal.is_none());
    assert_eq!(app.focus(), Pane::Title);
    assert_eq!(app.title.text, "b");
}

/// Escape used to leave the user with no feedback at all — the modal just
/// vanished. Pin that cancelling the rename-collision Guard now names what
/// it cancelled via a status message.
#[test]
fn escape_on_the_rename_collision_guard_sets_a_cancellation_status() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);
    send(&mut app, reply);

    send(&mut app, plain(KeyCode::Escape));

    assert_eq!(app.status_message.as_deref(), Some("rename cancelled"));
    assert_eq!(app.status_source, app::StatusSource::Other);
}

/// `r` on the guard for a `db: None` document cannot capture the displaced
/// bytes (§1.4.10), so the prompt stays up with an explanation and the disk
/// is untouched. The footer must not offer `[R]eplace` either.
#[test]
fn replace_is_refused_and_unoffered_without_a_store() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let reply = collide(&mut app, &mem);
    send(&mut app, reply);

    assert!(
        !footer::footer_text(&app).contains(banner::RENAME_REPLACE.label),
        "an option the app would refuse must not be offered"
    );

    send(&mut app, plain(KeyCode::Char('r')));

    assert!(matches!(app.rename, RenameState::Collision { .. }));
    assert!(app.modal.is_some(), "the prompt must stay up");
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|m| m.contains("cannot replace")),
        "got {:?}",
        app.status_message
    );
    assert_eq!(mem.read(Path::new("/root/a.md")).unwrap(), b"a content");
    assert_eq!(mem.read(Path::new("/root/b.md")).unwrap(), b"theirs");
}

/// A stale-generation `Msg::RenameDone` (dismissed, then restarted) must
/// leave the fresh state alone.
#[test]
fn a_stale_rename_reply_is_dropped() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    let mut first = rename_to(&mut app, "b");
    let stale = first
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");

    // Abandon it and start a fresh one under a new generation.
    app.rename = RenameState::Idle;
    let mut second = rename_to(&mut app, "c");
    assert!(matches!(app.rename, RenameState::Committing { .. }));
    let fresh_state = app.rename.clone();
    let _fresh_cmd = second
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a Rename Cmd");

    // The FIRST cmd's reply lands late.
    send(&mut app, stale.run().expect("a reply"));

    assert_eq!(
        app.rename, fresh_state,
        "a stale reply must not disturb the fresh rename"
    );
}

/// `close_now` on the renaming document while `Collision` clears both the
/// machine and its prompt.
#[test]
fn closing_the_renaming_document_clears_the_machine_and_the_prompt() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/c.md"), b"c content")
        .expect("seed c.md");
    let mut app = app_with(&mem);
    // A second document, so the last-document floor doesn't refuse.
    workspace::open_path(&mut app, Path::new("/root/c.md"));
    let first_tab = app.tabs.order[0];
    workspace::switch_to(&mut app, first_tab);
    let victim = app.active;

    let reply = collide(&mut app, &mem);
    send(&mut app, reply);
    assert!(matches!(app.rename, RenameState::Collision { .. }));

    workspace::close_now(&mut app, victim);

    assert_eq!(app.rename, RenameState::Idle);
    assert!(app.modal.is_none());
}

// ── The store-backed replace ────────────────────────────────────────────

/// The full `[R]eplace`: `r` enqueues an `OpKind::RenameReplace`, and the
/// ack binds the new path and reports the preserved bytes.
#[test]
fn replace_with_a_real_store_preserves_the_displaced_bytes() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"theirs")
        .expect("seed b.md");
    let (mut app, rx) = app_with_store(&mem);

    // Drive the collision through the store route.
    rename_to(&mut app, "b");
    assert!(matches!(app.rename, RenameState::Committing { .. }));
    let evt = next_event(&rx);
    send(&mut app, Msg::Db(evt));

    assert!(
        matches!(app.rename, RenameState::Collision { .. }),
        "expected a collision, got {:?}",
        app.rename
    );
    assert!(
        footer::footer_text(&app).contains(banner::RENAME_REPLACE.label),
        "a store-bound document must be offered [R]eplace"
    );

    let ops_before = app.db_ops.len();
    send(&mut app, plain(KeyCode::Char('r')));
    assert!(
        matches!(app.rename, RenameState::Capturing { .. }),
        "expected Capturing, got {:?}",
        app.rename
    );
    assert_eq!(app.db_ops.len(), ops_before + 1, "one replace op enqueued");
    assert!(app.modal.is_none(), "the prompt is resolved");

    let evt = next_event(&rx);
    send(&mut app, Msg::Db(evt));

    assert_eq!(app.rename, RenameState::Idle);
    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/b.md")));
    assert_eq!(mem.read(Path::new("/root/b.md")).unwrap(), b"a content");
    assert!(mem.read(Path::new("/root/a.md")).is_err());
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|m| m.contains("preserved")),
        "the status must say the replaced bytes were kept, got {:?}",
        app.status_message
    );
}

// ── Draft naming ────────────────────────────────────────────────────────

/// Enter on a pathless draft is a CREATE, not a rename: no `Rename` state
/// survives, and the file is published no-clobber.
#[test]
fn naming_a_draft_creates_the_file() {
    let mem = Arc::new(Mem::new());
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new("draft body"), None, vfs, None);

    send(&mut app, ctrl('r'));
    type_text(&mut app, "fresh");
    let mut effects = send(&mut app, plain(KeyCode::Enter));

    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a create Cmd");
    send(&mut app, cmd.run().expect("a reply"));

    assert_eq!(app.rename, RenameState::Idle);
    let path = active_path(&app).expect("the draft must now be bound");
    assert_eq!(path.file_name().unwrap(), "fresh.md");
    assert_eq!(mem.read(&path).unwrap(), b"draft body");
    assert_eq!(
        app.active_doc().file_name(),
        "fresh.md",
        "the no-store create ack must switch the title to the real filename"
    );
}

/// A draft name that collides gives a FOOTER refusal and never a
/// `RenameCollision` guard — offering `[R]eplace` would overwrite a foreign
/// file with a buffer that has no CAS baseline (§1.4.7).
#[test]
fn a_colliding_draft_name_refuses_in_the_footer_with_no_guard() {
    let mem = Arc::new(Mem::new());
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new("draft body"), None, vfs, None);
    // An absolute spelling: `Mem::resolve` now lexically normalizes
    // (WP1.S6), so a bare relative/dotted spelling here would be published
    // under a different key than this test's own closing `mem.read`
    // (which never resolves) looks up.
    let existing = Path::new("/taken.md");
    mem.save_atomic(existing, b"someone else's file")
        .expect("seed");

    send(&mut app, ctrl('r'));
    type_text(&mut app, "taken");
    let mut effects = send(&mut app, plain(KeyCode::Enter));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("a create Cmd");
    send(&mut app, cmd.run().expect("a reply"));

    assert_eq!(app.rename, RenameState::Idle);
    assert!(
        app.modal.is_none(),
        "a draft collision must never raise a guard"
    );
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|m| m.contains("already exists")),
        "got {:?}",
        app.status_message
    );
    assert!(
        active_path(&app).is_none(),
        "a refused create must leave the draft untitled (a later save must \
         not overwrite the winner)"
    );
    assert_eq!(mem.read(existing).unwrap(), b"someone else's file");
}

/// Regression: naming a store-bound draft (^R -> Enter, routed through
/// `save::bind_new_now`'s materialize) must switch the title to the real
/// filename via the SAME `Document::bind_path` chokepoint the no-store
/// route (`naming_a_draft_creates_the_file`, above) already goes through —
/// not just set `file_path` while leaving a stale `display_name` override
/// (e.g. "Untitled 1") to shadow it forever.
#[test]
fn store_bound_draft_create_ack_clears_the_untitled_display_name() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/seed.md"), b"seed")
        .expect("seed");
    let (mut app, rx) = draft_app_with_store(&mem);

    send(&mut app, ctrl('r'));
    type_text(&mut app, "fresh");
    send(&mut app, plain(KeyCode::Enter));

    // WP7: the store-backed create is now a three-hop round trip —
    // `MaterializePrepare`'s ack spawns the caller-side `vfs` `Cmd`
    // (`handle_prepare_ack`), which itself replies with a `Msg` that
    // enqueues `MaterializeRecord`.
    let prep_evt = next_event(&rx);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = next_event(&rx);
    send(&mut app, Msg::Db(record_evt));

    assert_eq!(app.rename, RenameState::Idle);
    let path = active_path(&app).expect("the draft must now be bound");
    assert_eq!(path.file_name().unwrap(), "fresh.md");
    assert_eq!(
        app.active_doc().file_name(),
        "fresh.md",
        "a store-bound create ack must clear the untitled display_name override"
    );
}

// ── WP2: focus loss is the single commit chokepoint ─────────────────────

/// Leaving the title for the Explorer (`^b`) must commit the pending rename
/// exactly like Enter does — the hoisted blur gate at the top of
/// `pane::handle_global_command` runs before the `FocusExplorer` arm.
#[test]
fn leaving_the_title_for_the_explorer_commits_the_rename() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    type_new_name(&mut app, "b");

    let mut effects = send(&mut app, ctrl('b'));

    assert_eq!(app.focus(), Pane::Explorer);
    let cmds: Vec<_> = effects
        .cmds
        .drain(..)
        .filter(|c| c.kind() == CmdKind::Rename)
        .collect();
    assert_eq!(
        cmds.len(),
        1,
        "leaving the title must commit the pending rename"
    );
    assert!(matches!(app.rename, RenameState::Committing { .. }));
}

/// Escape is an unconditional exit even while the typed name is invalid
/// (here, empty): it reverts FIRST, so there is nothing left for `on_blur`
/// to veto, and focus always releases.
#[test]
fn escape_releases_focus_even_when_the_typed_name_is_invalid() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    type_new_name(&mut app, "");

    let effects = send(&mut app, plain(KeyCode::Escape));

    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.title.text, "a", "reverted to the committed name");
    assert!(
        !effects.cmds.iter().any(|c| c.kind() == CmdKind::Rename),
        "Escape must never fire a rename"
    );
}

/// An invalid name (here, empty) vetoes the FOCUS change on Enter — the
/// user stays in the title with the reason already in the footer (decision
/// 7), rather than being bounced back to the Editor with an unresolved
/// name.
#[test]
fn an_invalid_name_vetoes_the_focus_change() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    type_new_name(&mut app, "");

    send(&mut app, plain(KeyCode::Enter));

    assert_eq!(
        app.focus(),
        Pane::Title,
        "a refused commit must not release focus"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("that name can't be used for a file")
    );
}

/// Gotcha 5: a vetoed blur must never block a global command reaching its
/// own arm — ⌘S still triggers a save, and `^c` twice still quits, even
/// while the title holds an unusable (empty) name.
#[test]
fn an_invalid_name_still_lets_the_user_quit_and_save() {
    let mem = seeded_vfs();
    let (mut app, _rx) = app_with_store(&mem);
    // A real edit, not `mark_dirty_from_hydration` — `trigger_save` gates on
    // `buffer.version() != saved_version`, which only an actual edit moves.
    send(&mut app, plain(KeyCode::Char('!')));
    type_new_name(&mut app, "");
    assert_eq!(
        app.focus(),
        Pane::Title,
        "test setup: the veto leaves focus in the title"
    );

    // ⌘S must still reach `Save`'s own arm rather than being swallowed by
    // the title: the hoisted gate only ever vetoes the FOCUS transition,
    // never the command itself.
    let cmd_s = Mods {
        sup: true,
        ..Mods::NONE
    };
    send(&mut app, key(KeyCode::Char('s'), cmd_s));
    assert!(
        app.active_doc().save_in_flight,
        "\u{2318}S must still trigger a save even with an unusable name pending"
    );

    // `^c` twice must still reach the quit chord's own arm and complete
    // quit — this document is store-bound, so the unpreserved-dirty Guard
    // gate never intercepts it.
    send(&mut app, ctrl('c'));
    send(&mut app, ctrl('c'));
    assert!(app.should_quit, "^c^c must still be able to quit");
}

/// WP2.S8 did not strand focus: both the Explorer's `Enter`
/// (`workspace::open_path`, wrapped by `explorer_keys::open_selected`) and
/// the Tabs pane's `Enter` (`opentabs::handle_key`'s `Select` arm) still
/// land focus on the Editor now that `switch_to` itself no longer writes
/// it.
#[test]
fn explorer_enter_and_tabs_enter_both_land_focus_on_the_editor() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);

    let mut effects = send(&mut app, ctrl('b'));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::ReadDir)
        .expect("a ReadDir Cmd");
    let msg = cmd.run().expect("a reply");
    send(&mut app, msg);
    assert_eq!(app.focus(), Pane::Explorer);
    let idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "b.md")
        .expect("b.md listed");
    app.explorer.nav.cursor = idx;

    send(&mut app, plain(KeyCode::Enter));
    assert_eq!(
        app.focus(),
        Pane::Editor,
        "Explorer Enter must land focus on the Editor"
    );

    send(&mut app, ctrl('t'));
    assert_eq!(app.focus(), Pane::Tabs);
    app.tabs.nav.cursor = 0;

    send(&mut app, plain(KeyCode::Enter));
    assert_eq!(
        app.focus(),
        Pane::Editor,
        "Tabs Enter must land focus on the Editor"
    );
}

/// Gotcha 6: `^1`-`^0` and `F1` fired from Explorer focus must land focus on
/// the Editor too — the hoisted blur gate at `pane.rs` fires only for
/// `Pane::Title`, so without WP2.S8's explicit `set_focus` in the
/// `TabSwitch`/`Help` arms, the document would switch while focus stayed
/// stranded on the chrome list.
#[test]
fn ctrl_1_and_f1_from_explorer_focus_land_focus_on_the_editor() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);
    workspace::open_path(&mut app, Path::new("/root/b.md"));

    send(&mut app, ctrl('b'));
    assert_eq!(app.focus(), Pane::Explorer);
    send(&mut app, ctrl('1'));
    assert_eq!(
        app.focus(),
        Pane::Editor,
        "^1 from Explorer focus must land focus on the Editor"
    );

    send(&mut app, ctrl('b'));
    assert_eq!(app.focus(), Pane::Explorer);
    send(&mut app, plain(KeyCode::F1));
    assert_eq!(
        app.focus(),
        Pane::Editor,
        "F1 from Explorer focus must land focus on the Editor"
    );
}

/// The ordering guard for decision 8: an uncommitted rename must target the
/// OUTGOING document, never the one about to become active. A different
/// document opening asynchronously (`workspace::open_path_async`, e.g. a
/// ctrl-click on a link) blurs the title — and so fires the pending rename
/// — BEFORE its `Msg::FileOpened` reply reassigns `app.active`.
#[test]
fn an_uncommitted_title_renames_the_outgoing_document_not_the_incoming_one() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/other.md"), b"other content")
        .expect("seed other.md");
    let mut app = app_with(&mem);

    type_new_name(&mut app, "renamed");

    let mut effects = Effects::default();
    workspace::open_path_async(&mut app, Path::new("/root/other.md"), None, &mut effects);
    let read_cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::ReadFile)
        .expect("a ReadFile Cmd");
    let file_opened = read_cmd.run().expect("a reply");
    let mut effects2 = send(&mut app, file_opened);

    // The active document is now the newly opened one...
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/other.md"))
    );
    // ...but the rename Cmd the blur fired targeted the OLD document's own
    // directory and old name, never the new one.
    let rename_cmd = effects2
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Rename)
        .expect("the blur must have fired a rename Cmd for the outgoing document");
    let reply = rename_cmd.run().expect("a reply");
    send(&mut app, reply);

    assert_eq!(
        mem.read(Path::new("/root/renamed.md")).unwrap(),
        b"a content"
    );
    assert!(
        mem.read(Path::new("/root/a.md")).is_err(),
        "the OLD name must be gone"
    );
    assert_eq!(
        mem.read(Path::new("/root/other.md")).unwrap(),
        b"other content",
        "the newly-opened document's own file must be untouched"
    );
}

/// Decision 8's conditional half: a failed Explorer open raises the error
/// banner and must NOT steal the keyboard — focus stays on the Explorer so
/// the user can try a different entry.
#[test]
fn a_failed_explorer_open_leaves_focus_on_the_explorer() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);

    let mut effects = send(&mut app, ctrl('b'));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::ReadDir)
        .expect("a ReadDir Cmd");
    let msg = cmd.run().expect("a reply");
    send(&mut app, msg);
    assert_eq!(app.focus(), Pane::Explorer);

    let idx = app
        .explorer
        .entries
        .iter()
        .position(|e| e.name == "b.md")
        .expect("b.md listed");
    app.explorer.nav.cursor = idx;
    mem.fail_next(rune_vfs::OpKind::Read, std::io::ErrorKind::PermissionDenied);

    send(&mut app, plain(KeyCode::Enter));

    assert_eq!(
        app.focus(),
        Pane::Explorer,
        "a failed open must not steal the keyboard from the Explorer"
    );
    assert!(
        app.modal.is_some(),
        "the read failure must raise the error banner"
    );
}

/// The first half of WP2.S8c's guard: closing the active document reseeds
/// the title from the document that becomes active in its place.
#[test]
fn closing_a_tab_reseeds_the_title_from_the_new_active_document() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);
    let first = app.active;
    workspace::open_path(&mut app, Path::new("/root/b.md"));
    let second = app.active;
    assert_ne!(first, second, "test setup: two distinct documents");

    workspace::close_now(&mut app, second);

    assert_eq!(app.active, first);
    assert_eq!(
        app.title.text, "a",
        "the title must reseed from the new active document"
    );
}

/// The second half of WP2.S8c's guard: `close_now` is the one active-
/// document reseed with no blur in front of it — an async close landing for
/// the very document being renamed must leave the typed name alone rather
/// than silently overwrite it.
#[test]
fn closing_a_background_tab_while_renaming_leaves_the_typed_name_alone() {
    let mem = seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let mut app = app_with(&mem);
    // A second document, so the last-document floor doesn't refuse.
    workspace::open_path(&mut app, Path::new("/root/b.md"));
    let first_tab = app.tabs.order[0];
    workspace::switch_to(&mut app, first_tab);
    let victim = app.active;

    type_new_name(&mut app, "zzz");

    // An async close for the very document being renamed lands with no
    // blur in front of it (mirrors `materialize_ack::close_if_pending`'s own
    // shape) — the typed name must survive untouched.
    workspace::close_now(&mut app, victim);

    assert_eq!(
        app.focus(),
        Pane::Title,
        "focus must not be silently displaced"
    );
    assert_eq!(
        app.title.text, "zzz",
        "the typed name must survive an async close of the document being renamed"
    );
}

/// Decision 12: the Help document is read-only, so its title can never gain
/// focus at all — `^r` refuses with a status instead, and the title row
/// still reads "Help".
#[test]
fn the_help_document_refuses_title_focus() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, plain(KeyCode::F1));
    assert_eq!(app.active_doc().file_name(), "Help");

    send(&mut app, ctrl('r'));

    assert_eq!(
        app.focus(),
        Pane::Editor,
        "a read-only document's title must never gain focus"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("this document is read-only")
    );
    assert_eq!(
        app.active_doc().file_name(),
        "Help",
        "the title row must still read Help"
    );
}
