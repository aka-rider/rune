//! Quit is a correlated continuation, not a
//! fire-and-forget prompt. Every scenario here is the reported wedge or one
//! of its direct corollaries — a dirty, unpreserved document (no live
//! recovery journal: `db: None`, or a store present but degraded) must
//! never leave `^C` stuck showing the same prompt forever, and answering
//! `[S]ave`/`[D]iscard` must always leave the app either quitting, saving,
//! or explaining why not.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod dirty_common;
mod quit_guard_common;

use rune_tui::app::update;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::KeyCode;
use rune_tui::pane::Pane;
use rune_tui::runtime::{CmdError, Effects, Msg, SaveOutcomeDetail};

use quit_guard_common::{ctrl_c, guard_kind, key, named_dirty_doc, press, test_app};

/// The exact reported wedge: a single dirty, unpreserved document (the
/// default shape — no file argument, no recovery journal), `^C` raises the
/// Guard, and `[D]iscard` must actually quit — a past bug answered
/// this by silently closing the document instead (defect 2, "the guard is
/// impossible to exit from").
#[test]
fn single_dirty_unpreserved_document_ctrl_c_guard_discard_quits() {
    let mut app = test_app();
    let id = app.active;
    dirty_common::force_dirty(&mut app, id);
    assert!(
        !app.doc(id).unwrap().is_store_bound(),
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
        app.doc(id).unwrap().path().is_none(),
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
        app.quit.fan_out().is_none(),
        "a refused save must never leave a quit intent waiting on it"
    );

    // The guard must not simply re-raise on the very next tick with nothing
    // changed — the fuzz invariant also checks this.
    assert!(app.guard.is_none());
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
        app.quit.fan_out().map(|i| i.pending.len()),
        Some(1),
        "the fan-out must be waiting on exactly this document"
    );

    let ticket = app.doc(id).unwrap().save_ticket().unwrap();
    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            ticket,
            version,
            result: Ok(()),
            detail: SaveOutcomeDetail {
                durable: true,
                stray_temp: None,
                race: None,
            },
        },
        &mut effects,
    );

    assert!(
        app.should_quit,
        "the matching successful ack must complete the quit"
    );
    assert!(app.quit.fan_out().is_none());
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
    assert!(app.quit.fan_out().is_some());

    let ticket = app.doc(id).unwrap().save_ticket().unwrap();
    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id,
            ticket,
            version,
            result: Err(CmdError::Refused("disk full".to_string())),
            detail: SaveOutcomeDetail {
                durable: true,
                stray_temp: None,
                race: None,
            },
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
        app.quit.fan_out().is_none(),
        "a failed save must abort the whole quit intent, not just this document's entry"
    );
}
