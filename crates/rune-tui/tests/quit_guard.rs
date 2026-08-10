//! Plan WP2 "Done when" tests: quit is a correlated continuation, not a
//! fire-and-forget prompt. Every scenario here is the reported wedge or one
//! of its direct corollaries — a dirty, unpreserved document (no live
//! recovery journal: `db: None`, or a store present but degraded) must
//! never leave `^C` stuck showing the same prompt forever, and answering
//! `[S]ave`/`[D]iscard` must always leave the app either quitting, saving,
//! or explaining why not.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod dirty_common;

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{ClockFn, Store};
use rune_tui::app::{App, update};
use rune_tui::db::{Db, DocDb};
use rune_tui::document::DocumentId;
use rune_tui::guard::{GuardKind, GuardPrompt};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::{Mem, Vfs};

fn test_app() -> App {
    App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
}

fn key(code: KeyCode) -> KeyInput {
    KeyInput {
        code,
        mods: Mods::NONE,
    }
}

fn ctrl_c() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('c'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    }
}

fn press(app: &mut App, input: KeyInput) {
    let mut effects = Effects::default();
    update(app, Msg::Key(input), &mut effects);
}

fn guard_kind(app: &App) -> Option<&GuardKind> {
    match &app.guard {
        Some(GuardPrompt { kind, .. }) => Some(kind),
        None => None,
    }
}

/// The exact reported wedge: a single dirty, unpreserved document (the
/// default shape — no file argument, no recovery journal), `^C` raises the
/// Guard, and `[D]iscard` must actually quit — the pre-WP2 bug answered
/// this by silently closing the document instead (defect 2, "the guard is
/// impossible to exit from").
#[test]
fn single_dirty_unpreserved_document_ctrl_c_guard_discard_quits() {
    let mut app = test_app();
    let id = app.active;
    dirty_common::force_dirty(&mut app, id);
    assert!(
        app.doc(id).unwrap().db.is_none(),
        "test setup: no db binding"
    );

    press(&mut app, ctrl_c());
    assert_eq!(
        guard_kind(&app),
        Some(&GuardKind::DirtyQuit),
        "a dirty, unpreserved document must raise the DirtyQuit guard on the first ^C"
    );

    press(&mut app, key(KeyCode::Char('d')));
    assert!(
        app.should_quit,
        "[D]iscard on the quit guard must actually quit, not merely close"
    );
    assert!(app.guard.is_none());
}

/// A pathless draft has nothing to save TO yet: `[S]ave` must route to the
/// same "name it" flow `trigger_save` already implements, never wedge a
/// quit intent waiting on a save that will never start.
#[test]
fn pathless_draft_guard_save_focuses_the_title_and_abandons_the_quit_intent() {
    let mut app = test_app();
    let id = app.active;
    dirty_common::force_dirty(&mut app, id);
    assert!(
        app.doc(id).unwrap().file_path.is_none(),
        "test setup: pathless"
    );

    press(&mut app, ctrl_c());
    assert!(guard_kind(&app).is_some());

    press(&mut app, key(KeyCode::Char('s')));

    assert!(app.guard.is_none(), "the guard must clear either way");
    assert_eq!(app.focus(), Pane::Title, "naming flow must focus the title");
    assert!(
        rune_tui::messages::newest_text(&app)
            .is_some_and(|m| m.contains("name this document to save it")),
        "status was {:?}",
        rune_tui::messages::newest_text(&app)
    );
    assert!(!app.should_quit, "nothing was actually saved yet");
    assert!(
        app.quit_intent.is_none(),
        "a refused save must never leave a quit intent waiting on it"
    );

    // The guard must not simply re-raise on the very next tick with nothing
    // changed — the new fact WP2's fuzz invariant also checks.
    assert!(app.guard.is_none());
}

fn named_dirty_doc(app: &mut App, path: &str) -> DocumentId {
    let id = app.active;
    app.doc_mut(id).unwrap().bind_path(PathBuf::from(path));
    dirty_common::force_dirty(app, id);
    id
}

/// A named, unpreserved dirty document: `[S]ave` starts the no-store
/// fallback save, and only the matching `SaveDone` ack completes the quit.
#[test]
fn named_dirty_doc_guard_save_completes_quit_on_a_successful_ack() {
    let mut app = test_app();
    let id = named_dirty_doc(&mut app, "/a.md");
    let version = app.doc(id).unwrap().buffer.version();

    press(&mut app, ctrl_c());
    press(&mut app, key(KeyCode::Char('s')));

    assert!(
        app.doc(id).unwrap().save_in_flight(),
        "save must have started"
    );
    assert!(!app.should_quit);
    assert_eq!(
        app.quit_intent.as_ref().map(|i| i.pending.len()),
        Some(1),
        "the fan-out must be waiting on exactly this document"
    );

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Ok(()),
            durable: true,
        },
        &mut effects,
    );

    assert!(
        app.should_quit,
        "the matching successful ack must complete the quit"
    );
    assert!(app.quit_intent.is_none());
}

/// The converse: a FAILED ack must abort the quit outright rather than
/// exit over a save the user believes succeeded.
#[test]
fn named_dirty_doc_guard_save_failing_ack_aborts_the_quit() {
    let mut app = test_app();
    let id = named_dirty_doc(&mut app, "/a.md");
    let version = app.doc(id).unwrap().buffer.version();

    press(&mut app, ctrl_c());
    press(&mut app, key(KeyCode::Char('s')));
    assert!(app.quit_intent.is_some());

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            version,
            result: Err("disk full".to_string()),
            durable: true,
        },
        &mut effects,
    );

    assert!(
        !app.should_quit,
        "a failed save must never let quit complete"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some(),
        "the failure must be surfaced, not swallowed"
    );
    assert!(
        app.quit_intent.is_none(),
        "a failed save must abort the whole quit intent, not just this document's entry"
    );
}

/// Two dirty, unpreserved documents: `[S]ave` must fan out to BOTH, and
/// quit must only complete once EVERY one of them has acked — not the
/// first.
#[test]
fn two_dirty_docs_guard_save_quits_only_after_both_ack() {
    let mut app = test_app();
    let id_a = named_dirty_doc(&mut app, "/a.md");
    let id_b = app.open_document(Buffer::new("second"));
    app.doc_mut(id_b).unwrap().bind_path(PathBuf::from("/b.md"));
    dirty_common::force_dirty(&mut app, id_b);
    let version_a = app.doc(id_a).unwrap().buffer.version();
    let version_b = app.doc(id_b).unwrap().buffer.version();

    press(&mut app, ctrl_c());
    press(&mut app, key(KeyCode::Char('s')));

    assert_eq!(app.quit_intent.as_ref().map(|i| i.pending.len()), Some(2));
    assert!(app.doc(id_a).unwrap().save_in_flight());
    assert!(app.doc(id_b).unwrap().save_in_flight());

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id: id_a,
            version: version_a,
            result: Ok(()),
            durable: true,
        },
        &mut effects,
    );
    assert!(
        !app.should_quit,
        "one ack out of two must not complete the quit"
    );

    update(
        &mut app,
        Msg::SaveDone {
            id: id_b,
            version: version_b,
            result: Ok(()),
            durable: true,
        },
        &mut effects,
    );
    assert!(
        app.should_quit,
        "the second, final ack must complete the quit"
    );
}

/// One of the two awaited documents is closed mid-flight instead of
/// acking (e.g. a separate `[D]iscard` on its own close-guard) — the quit
/// intent must still resolve once the OTHER document's save lands, rather
/// than stranding forever on an entry that will never ack.
#[test]
fn closing_one_awaited_document_mid_flight_still_lets_the_quit_resolve() {
    let mut app = test_app();
    let id_a = named_dirty_doc(&mut app, "/a.md");
    let id_b = app.open_document(Buffer::new("second"));
    app.doc_mut(id_b).unwrap().bind_path(PathBuf::from("/b.md"));
    dirty_common::force_dirty(&mut app, id_b);
    let version_a = app.doc(id_a).unwrap().buffer.version();

    press(&mut app, ctrl_c());
    press(&mut app, key(KeyCode::Char('s')));
    assert_eq!(app.quit_intent.as_ref().map(|i| i.pending.len()), Some(2));

    let mut effects = Effects::default();
    let _ = rune_tui::workspace::close_now(&mut app, id_b, &mut effects);
    assert!(
        !app.should_quit,
        "closing one awaited document must not itself complete the quit \
         while the other is still outstanding"
    );
    assert_eq!(app.quit_intent.as_ref().map(|i| i.pending.len()), Some(1));

    update(
        &mut app,
        Msg::SaveDone {
            id: id_a,
            version: version_a,
            result: Ok(()),
            durable: true,
        },
        &mut effects,
    );
    assert!(
        app.should_quit,
        "the remaining document's ack must still complete the quit"
    );
}

/// A whole-store failure landing mid quit-save must abort the quit (never
/// exit over a save the user believes succeeded) AND leave the state clean
/// enough that the very next `^C` still works — the strand `on_store_
/// failure`'s own doc comment warns against.
#[test]
fn store_failure_mid_quit_save_aborts_the_quit_and_the_next_ctrl_c_still_works() {
    let mut app = test_app();
    let id = named_dirty_doc(&mut app, "/a.md");

    press(&mut app, ctrl_c());
    press(&mut app, key(KeyCode::Char('s')));
    assert!(app.quit_intent.is_some());

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::Db(rune_db::DbEvent::Fatal {
            error: "writer panicked".to_string(),
        }),
        &mut effects,
    );

    assert!(
        !app.should_quit,
        "a store failure must never let quit complete"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some(),
        "the failure must be surfaced"
    );
    assert!(
        app.quit_intent.is_none(),
        "the stranded intent must be cleared"
    );
    assert!(!app.doc(id).unwrap().save_in_flight());

    // The next `^C` must still raise a fresh, resolvable guard rather than
    // silently doing nothing (the document is still genuinely dirty).
    press(&mut app, ctrl_c());
    assert_eq!(guard_kind(&app), Some(&GuardKind::DirtyQuit));
}

/// Code-review finding 6: with a DEGRADED store (every dirty document is
/// unpreserved — `App::is_preserved` is false for all of them, so the
/// fan-out set is "every dirty document"), each `trigger_save` call that
/// reaches the degraded arm only ARMS `App::pending_save_confirm` — a
/// single global slot, not one per document. Two dirty documents must
/// therefore not both attempt to arm it: the second attempt would silently
/// overwrite the first's gate and leave the status naming whichever
/// document happened to go last, while queuing a redundant confirm-timeout
/// `Cmd` for a gate that no longer matches its own document. The fan-out
/// must stop at the first arm, leaving exactly one confirm gate, one
/// coherent (document-naming) status, and no quit intent stranded waiting
/// on a save that never started.
#[test]
fn two_dirty_docs_degraded_store_arms_exactly_one_confirm_gate() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), Box::new(|_evt| {}))
        .expect("open in-memory store");
    let bridge = rune_tui::db::DbBridge::bootstrap();
    let db = Db::new(store, bridge, true); // degraded from the start

    let mut app = App::new(
        Buffer::new("hello"),
        Some(PathBuf::from("/a.md")),
        vfs,
        Some(db),
    );
    let id_a = app.active;
    app.doc_mut(id_a).unwrap().db = Some(DocDb::new(1, false, 0));
    dirty_common::force_dirty(&mut app, id_a);

    let id_b = app.open_document(Buffer::new("second"));
    app.doc_mut(id_b).unwrap().bind_path(PathBuf::from("/b.md"));
    app.doc_mut(id_b).unwrap().db = Some(DocDb::new(2, false, 0));
    dirty_common::force_dirty(&mut app, id_b);

    press(&mut app, ctrl_c());
    assert_eq!(guard_kind(&app), Some(&GuardKind::DirtyQuit));
    press(&mut app, key(KeyCode::Char('s')));

    assert!(
        app.pending_save_confirm.is_some(),
        "exactly one confirm gate must be armed"
    );
    let (armed_id, _) = app.pending_save_confirm.expect("checked above");
    assert!(
        armed_id == id_a || armed_id == id_b,
        "the armed gate must name one of the two dirty documents"
    );
    assert!(
        !app.doc(id_a).unwrap().save_in_flight() && !app.doc(id_b).unwrap().save_in_flight(),
        "the degraded arm must never enqueue a save on its first press"
    );
    let expected_name = if armed_id == id_a { "a.md" } else { "b.md" };
    assert!(
        rune_tui::messages::newest_text(&app)
            .is_some_and(|m| m.contains("recovery disabled") && m.contains(expected_name)),
        "the status must name the SAME document the confirm gate is armed for, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
    assert!(
        app.quit_intent.is_none(),
        "no save actually started, so no quit intent may be left waiting"
    );
    assert!(!app.should_quit);
}
