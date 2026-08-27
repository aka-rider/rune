//! Regression for the `SavePhase::Publishing` permanent wedge: a `^S` whose
//! `vfs` reply lands as a disk `Conflict` (the CAS baseline the caller
//! captured no longer matches disk) calls `record_outcome(published:
//! false)`. If `rune-db`'s own `materialize_record` enqueue then fails
//! (writer thread dead), the old `Err` arm only resolved the document's
//! `SaveState` when `published` was `true` — an unpublished `Conflict`
//! fell through to `on_store_failure`'s degrade sweep, whose
//! `SavePhase::Publishing` arm was a silent no-op. `begin_recording` was
//! never called, so `save_in_flight()` stayed `true` forever: every later
//! `^S` refused ("a save is already in progress"), with no way out except
//! discarding the tab.
//!
//! Driven through `rune_fuzz::Session` — never by poking `App` fields
//! directly. `Action::DivergeDisk` supplies the external disk write that
//! turns the reply into a `Conflict`; `Store::kill_writer_for_test` +
//! `probe_blocking_for_test` kill the writer thread deterministically,
//! mirroring `materialize_dead_writer_reentrancy.rs`'s own fixture.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rune_fuzz::Session;
use rune_fuzz::action::Action;
use rune_tui::document::SavePhase;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

const SAVE: KeyInput = KeyInput {
    code: KeyCode::Char('s'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

fn wait_for_writer_death(store: &rune_db::Store, doc_id: i64) {
    let max_attempts = 4 * rune_db::QUEUE_DEPTH;
    for attempt in 0..=max_attempts {
        match store.probe_blocking_for_test(rune_db::DocId(doc_id)) {
            Ok(_) => assert!(
                attempt < max_attempts,
                "writer never confirmed dead after {max_attempts} blocking probes"
            ),
            Err(rune_db::Error::WriterGone) => return,
            Err(e) => panic!("unexpected error while awaiting writer death: {e}"),
        }
    }
}

/// Drives the document into `Publishing` with its vfs `Cmd` spawned but not
/// yet run, having already diverged the disk underneath it and killed the
/// store's writer thread — the shared setup for both wedge tests below.
fn publishing_with_dead_writer_and_diverged_disk(session: &mut Session) {
    let id = session.app().active;

    assert!(session.type_("!").is_none());
    assert!(session.key(SAVE).is_none());
    assert_eq!(
        session.app().doc(id).unwrap().save_phase(),
        SavePhase::Preparing,
        "test setup: ^S must start a MaterializePrepare enqueue"
    );

    assert!(
        session.deliver_db_all().is_none(),
        "the prepare ack must advance the document to Publishing and spawn \
         its vfs Cmd"
    );
    assert_eq!(
        session.app().doc(id).unwrap().save_phase(),
        SavePhase::Publishing
    );

    assert!(
        session.act(Action::DivergeDisk).is_none(),
        "an external write must land on disk before the vfs Cmd runs its \
         own CAS check"
    );

    let db_id = session.app().doc(id).unwrap().doc_db().unwrap().db_id;
    let store = &session.app().db.as_ref().unwrap().store;
    store.kill_writer_for_test().expect("enqueue the kill op");
    wait_for_writer_death(store, db_id);
    assert!(
        !session.app().db.as_ref().unwrap().degraded,
        "test setup: the store must still read non-degraded going into the \
         vfs Cmd's reply"
    );
}

#[test]
fn a_conflict_ack_whose_materialize_record_enqueue_fails_still_resolves_the_save() {
    let mut session = Session::open("/doc.md", "hello");
    let id = session.app().active;

    publishing_with_dead_writer_and_diverged_disk(&mut session);

    assert!(
        session.deliver().is_none(),
        "the vfs Cmd's Conflict outcome must settle without a violation"
    );

    assert_eq!(
        session.app().doc(id).unwrap().save_phase(),
        SavePhase::Idle,
        "a Conflict ack whose own materialize_record enqueue failed must \
         resolve the SaveState instead of leaving it wedged in Publishing"
    );
    assert!(
        !session.app().doc(id).unwrap().save_in_flight(),
        "the wedge under test: save_in_flight() must never stay true forever"
    );
    assert!(
        session.app().db.as_ref().unwrap().degraded,
        "a materialize_record enqueue failure must still degrade the store"
    );
    assert!(
        rune_tui::messages::newest_text(session.app()).is_some(),
        "the failure must be surfaced to the user, never silent"
    );

    assert!(session.type_("?").is_none());
    assert!(session.key(SAVE).is_none());
    assert!(
        !session.app().doc(id).unwrap().save_in_flight(),
        "test setup: a save into a freshly degraded store asks for an \
         explicit confirmation before it starts (unrelated to the wedge \
         under test)"
    );
    assert!(
        session.key(SAVE).is_none(),
        "the confirming ^S must be accepted, never refused as \"already in \
         progress\" — the wedge under test would have stuck save_in_flight() \
         at true from the very first failed ack, long before this point"
    );
    assert!(
        session.app().doc(id).unwrap().save_in_flight(),
        "the confirmed save must actually start"
    );
}

#[test]
fn a_path_disagreement_ack_resolves_the_save_without_degrading_the_store() {
    let mut session = Session::open("/doc.md", "hello");
    let id = session.app().active;

    assert!(session.type_("!").is_none());
    assert!(session.key(SAVE).is_none());
    assert!(session.deliver_db_all().is_none());
    assert_eq!(
        session.app().doc(id).unwrap().save_phase(),
        SavePhase::Publishing
    );

    let msg = {
        let doc = session.app().doc(id).unwrap();
        rune_tui::runtime::Msg::MaterializeVfsDone {
            id,
            ticket: doc.save_ticket().unwrap(),
            db_id: doc.doc_db().unwrap().db_id,
            seq: doc.doc_db().unwrap().last_known_seq.0,
            content: std::sync::Arc::from("!hello"),
            outcome: rune_tui::materialize_ack::MaterializeVfsOutcome::PathDisagreement,
        }
    };
    let mut effects = rune_tui::runtime::Effects::default();
    rune_tui::app::update(session.app_mut(), msg, &mut effects);

    assert_eq!(
        session.app().doc(id).unwrap().save_phase(),
        SavePhase::Idle,
        "a PathDisagreement ack must resolve this document's own SaveState"
    );
    assert!(!session.app().doc(id).unwrap().save_in_flight());
    assert!(
        !session.app().db.as_ref().unwrap().degraded,
        "a caller-bug guard on ONE save attempt must never degrade the \
         whole recovery store"
    );
    assert!(rune_tui::messages::newest_text(session.app()).is_some());
}
