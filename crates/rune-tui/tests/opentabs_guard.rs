//! Tests for the close guard's three resolutions (`[S]ave`, `[D]iscard`,
//! `Esc`), driven against a `Mem` vfs seeded with two files. This is the
//! 500-line-budget split of the original `opentabs.rs`: Tabs-pane-local
//! rendering/switching lives in the sibling `opentabs.rs`; the GLOBAL `^w`/
//! `^1`-`^0` binding tests live in `opentabs_global.rs`; all three pull
//! shared fixtures from `opentabs_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod opentabs_common;

use rune_tui::app;
use rune_tui::commands::edit;
use rune_tui::generation::Generation;
use rune_tui::keymap::KeyCode;
use rune_tui::runtime::{CmdError, CmdKind, Effects, Msg, SaveOutcomeDetail};
use rune_tui::workspace;

use opentabs_common::{open_second, open_seeded, plain};

/// `request_close` on a dirty document arms the Guard modal, and — like
/// the Error banner before it — every key is consumed at stage 1 while
/// it's up: it never reaches the editor's own buffer.
#[test]
fn request_close_on_a_dirty_doc_arms_the_guard_and_blocks_other_keys() {
    let mut session = open_seeded();
    let second = open_second(&mut session);
    edit::insert_char(session.app_mut(), second, '!');
    assert!(session.app().doc(second).unwrap().is_dirty());

    workspace::request_close(session.app_mut(), second, &mut Effects::default());
    assert!(
        session.app().guard.is_some(),
        "a dirty close must arm a modal"
    );

    let before = session
        .app()
        .doc(second)
        .unwrap()
        .buffer
        .content()
        .to_string();
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Key(plain(KeyCode::Char('q'))),
        &mut effects,
    );

    assert_eq!(
        session.app().doc(second).unwrap().buffer.content(),
        before,
        "a key consumed by the Guard must never reach commands::edit"
    );
    assert!(
        session.app().guard.is_some(),
        "an unbound key must leave the Guard up"
    );
    assert!(
        session.app().documents.contains_key(&second),
        "must not close yet"
    );
}

/// `[D]iscard` closes the document immediately and activates its neighbor.
#[test]
fn discard_closes_and_activates_the_neighbor() {
    let mut session = open_seeded();
    let first = session.app().active;
    let second = open_second(&mut session);
    edit::insert_char(session.app_mut(), second, '!');
    assert_eq!(session.app().active, second);

    workspace::request_close(session.app_mut(), second, &mut Effects::default());
    assert!(session.app().guard.is_some());
    // A degraded-save confirm gate armed for `second` and left unresolved
    // (review fix: `close_now` must sweep it, not just `pending_close_on_
    // save`) — must not survive the close as a dangling reference to a
    // document that no longer exists.
    session.app_mut().pending_save_confirm = Some((second, Generation::ZERO));

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Key(plain(KeyCode::Char('d'))),
        &mut effects,
    );

    assert!(session.app().guard.is_none());
    assert!(
        !session.app().documents.contains_key(&second),
        "b.md must be closed"
    );
    assert_eq!(session.app().documents.len(), 1);
    assert_eq!(
        session.app().active,
        first,
        "the sole remaining document takes over"
    );
    assert!(!session.app().documents.order().contains(&second));
    assert!(
        session.app().pending_save_confirm.is_none(),
        "a pending_save_confirm targeting the closed doc must be cleared too"
    );
}

/// `[S]ave` triggers a save, closing only once its `Msg::SaveDone` ack
/// reports success — never before, and never on a failure (mind: `db:
/// None` documents take the `SaveDone` fallback
/// path, exercised here since `open_seeded` builds an `App` with no store).
#[test]
fn save_then_close_waits_for_the_save_done_ack() {
    let mut session = open_seeded();
    if let Some(db) = session.app_mut().db.take() {
        db.shutdown();
    }
    let second = open_second(&mut session);
    edit::insert_char(session.app_mut(), second, '!');
    assert!(session.app().doc(second).unwrap().is_dirty());

    workspace::request_close(session.app_mut(), second, &mut Effects::default());
    assert!(session.app().guard.is_some());

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Key(plain(KeyCode::Char('s'))),
        &mut effects,
    );

    assert!(
        session.app().guard.is_none(),
        "the Guard clears the moment Save fires"
    );
    assert_eq!(
        session.app().pending_close_on_save,
        Some(second),
        "a save must actually have started"
    );
    assert!(
        session.app().documents.contains_key(&second),
        "must not close before the save's ack lands"
    );
    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::Save);

    let version = session.app().doc(second).unwrap().buffer.version();
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("the Save Cmd replies with a Msg");
    // Sanity: the driver really did produce `SaveDone` for `second`.
    match &msg {
        Msg::SaveDone { id, .. } => assert_eq!(*id, second),
        other => panic!("expected Msg::SaveDone, got {other:?}"),
    }

    let mut effects2 = Effects::default();
    app::update(session.app_mut(), msg, &mut effects2);

    assert!(
        !session.app().documents.contains_key(&second),
        "must close once the save's ack reports success"
    );
    assert_eq!(session.app().pending_close_on_save, None);
    let _ = version;
}

/// A failed `SaveDone` ack must NOT close the document — data safety over
/// honoring a stale close intent.
#[test]
fn a_failed_save_ack_leaves_the_document_open() {
    let mut session = open_seeded();
    if let Some(db) = session.app_mut().db.take() {
        db.shutdown();
    }
    let second = open_second(&mut session);
    edit::insert_char(session.app_mut(), second, '!');

    workspace::request_close(session.app_mut(), second, &mut Effects::default());
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Key(plain(KeyCode::Char('s'))),
        &mut effects,
    );
    assert_eq!(session.app().pending_close_on_save, Some(second));

    let version = session.app().doc(second).unwrap().buffer.version();
    let ticket = session.app().doc(second).unwrap().save_ticket().unwrap();
    let mut effects2 = Effects::default();
    app::update(
        session.app_mut(),
        Msg::SaveDone {
            id: second,
            ticket,
            version,
            result: Err(CmdError::Refused("disk full".to_string())),
            detail: SaveOutcomeDetail {
                durable: true,
                stray_temp: None,
                race: None,
            },
        },
        &mut effects2,
    );

    assert!(
        session.app().documents.contains_key(&second),
        "a failed save must never close the document"
    );
    assert_eq!(session.app().pending_close_on_save, None);
}

/// `Esc` cancels the Guard, leaving the document and its content untouched.
#[test]
fn escape_cancels_the_guard() {
    let mut session = open_seeded();
    let second = open_second(&mut session);
    edit::insert_char(session.app_mut(), second, '!');
    let content_before = session
        .app()
        .doc(second)
        .unwrap()
        .buffer
        .content()
        .to_string();

    workspace::request_close(session.app_mut(), second, &mut Effects::default());
    assert!(session.app().guard.is_some());

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Key(plain(KeyCode::Escape)),
        &mut effects,
    );

    assert!(session.app().guard.is_none());
    assert!(session.app().documents.contains_key(&second));
    assert_eq!(
        session.app().doc(second).unwrap().buffer.content(),
        content_before
    );
    assert!(
        session.app().doc(second).unwrap().is_dirty(),
        "still dirty, untouched"
    );
}

/// Escape used to leave the user with no feedback at all — the modal just
/// vanished. Pin that cancelling the dirty-close Guard now names what it
/// cancelled via a status message.
#[test]
fn escape_on_the_dirty_close_guard_sets_a_cancellation_status() {
    let mut session = open_seeded();
    let second = open_second(&mut session);
    edit::insert_char(session.app_mut(), second, '!');

    workspace::request_close(session.app_mut(), second, &mut Effects::default());
    assert!(session.app().guard.is_some());

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Key(plain(KeyCode::Escape)),
        &mut effects,
    );

    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("close cancelled")
    );
}

/// Closing the last remaining document mints a fresh untitled
/// draft rather than refusing — the old refusal made the user open another
/// document just to close the untitled one before they could leave.
#[test]
fn closing_the_only_document_mints_a_fresh_untitled_instead_of_refusing() {
    let mut session = open_seeded();
    let only = session.app().active;
    assert_eq!(session.app().documents.len(), 1);

    workspace::request_close(session.app_mut(), only, &mut Effects::default());

    assert!(
        session.app().guard.is_none(),
        "a clean close never arms a Guard"
    );
    assert!(
        !session.app().documents.contains_key(&only),
        "the original document must actually be gone"
    );
    assert_eq!(
        session.app().documents.len(),
        1,
        "closing the last document leaves exactly one — a fresh untitled"
    );
    assert_eq!(
        session.app().active_doc().display_name.as_deref(),
        Some("Untitled 1"),
        "the replacement is the fresh untitled draft, and it's now active"
    );
    assert!(
        rune_tui::messages::newest_text(session.app()).is_none(),
        "there is no more \"can't close\" refusal to report"
    );
}

/// The dirty variant of the same scenario still routes through the close
/// Guard — `^W` on a dirty-and-only document must not silently discard it.
/// `[D]iscard` then lands on the same fresh-untitled replacement.
#[test]
fn closing_a_dirty_only_document_still_routes_through_the_guard() {
    let mut session = open_seeded();
    let only = session.app().active;
    edit::insert_char(session.app_mut(), only, '!');
    assert!(session.app().doc(only).unwrap().is_dirty());

    workspace::request_close(session.app_mut(), only, &mut Effects::default());

    assert!(
        session.app().guard.is_some(),
        "a dirty close still arms the Guard, even for the only document"
    );
    assert!(
        session.app().documents.contains_key(&only),
        "not closed yet"
    );
    assert_eq!(session.app().documents.len(), 1);

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Key(plain(KeyCode::Char('d'))),
        &mut effects,
    );

    assert!(session.app().guard.is_none());
    assert!(!session.app().documents.contains_key(&only));
    assert_eq!(session.app().documents.len(), 1);
    assert_eq!(
        session.app().active_doc().display_name.as_deref(),
        Some("Untitled 1")
    );
}

/// A prior error message must never block a Guard from being raised:
/// errors are a non-modal log entry now, orthogonal to the Guard slot
/// — unlike a past modal error banner, which used to outrank and refuse a
/// lower-priority Guard request.
#[test]
fn an_error_message_never_blocks_a_guard_from_being_raised() {
    let mut session = open_seeded();
    let second = open_second(&mut session);
    edit::insert_char(session.app_mut(), second, '!');

    rune_tui::messages::error(session.app_mut(), "boom");
    assert_eq!(rune_tui::messages::newest_text(session.app()), Some("boom"));

    workspace::request_close(session.app_mut(), second, &mut Effects::default());

    assert!(
        matches!(
            session.app().guard,
            Some(ref prompt) if prompt.kind == rune_tui::guard::GuardKind::DirtyClose
        ),
        "a prior error message must never block a Guard from being raised"
    );
}

/// A cancellation ack must never cost the user an unacknowledged save
/// failure — the log is append-only, so an unrelated
/// cancellation posts its own entry without ever touching an earlier one.
#[test]
fn escape_on_a_guard_keeps_an_unacknowledged_save_failure_in_the_log() {
    let mut session = open_seeded();
    let second = open_second(&mut session);
    edit::insert_char(session.app_mut(), second, '!');
    rune_tui::messages::error(session.app_mut(), "save failed: disk full");

    workspace::request_close(session.app_mut(), second, &mut Effects::default());
    assert!(
        session.app().guard.is_some(),
        "a dirty close arms the Guard"
    );

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Key(plain(KeyCode::Escape)),
        &mut effects,
    );

    assert!(
        session.app().guard.is_none(),
        "Escape still cancels the Guard"
    );
    assert_eq!(
        rune_tui::messages::log_text(session.app()),
        "save failed: disk full\nclose cancelled",
        "the save failure must survive an unrelated cancellation, in order"
    );
}
