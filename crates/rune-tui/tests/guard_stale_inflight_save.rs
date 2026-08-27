//! Regression for the stale-in-flight-save data-loss defect: `^S` starts a
//! save capturing version V1, the user keeps typing (V2, in RAM only), then
//! asks to quit or close and answers `[S]ave` on the resulting Guard. Both
//! `start_quit_save_fan_out` and `handle_dirty_close_key` used to read
//! `save_in_flight`/`pending_save_version` only AFTER calling `trigger_save`
//! again, which cannot tell "a save already running before this press" apart
//! from "a save THIS press just started" — both return `SaveStart::
//! InFlight`. That enrolled the STALE V1 ack as what quit/close was
//! waiting on; once it landed, `quit_if_pending`/`close_if_pending`
//! (`materialize_ack/reactions.rs`) saw a version match and quit/closed,
//! discarding the V1->V2 edits that were only ever in RAM.
//!
//! Driven through `rune_fuzz::Session`'s real update seam — `key`/`type_`/
//! `deliver` — never by poking `App` fields directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_fuzz::Session;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

const SAVE: KeyInput = KeyInput {
    code: KeyCode::Char('s'),
    mods: CTRL,
};

const QUIT: KeyInput = KeyInput {
    code: KeyCode::Char('c'),
    mods: CTRL,
};

const CLOSE: KeyInput = KeyInput {
    code: KeyCode::Char('w'),
    mods: CTRL,
};

const GUARD_SAVE: KeyInput = KeyInput {
    code: KeyCode::Char('s'),
    mods: Mods::NONE,
};

fn guard_kind(session: &Session) -> Option<&GuardKind> {
    session.app().guard.as_ref().map(|prompt| &prompt.kind)
}

/// `^C` `s` on a Guard whose document already has ITS OWN earlier save in
/// flight must abandon the whole quit intent rather than wait on that
/// earlier save's version — the fix under test.
#[test]
fn quit_save_fan_out_never_quits_over_a_stale_save_ack() {
    let mut session = Session::open("/doc.md", "hello");
    let id = session.app().active;
    if let Some(db) = session.app_mut().db.take() {
        db.shutdown();
    }

    assert!(session.type_("X").is_none());
    assert!(session.key(SAVE).is_none());
    assert!(
        session.app().doc(id).unwrap().save_in_flight(),
        "test setup: the first ^S must have actually started a save"
    );

    assert!(session.type_("Y").is_none());
    let dirty_content = session.app().doc(id).unwrap().buffer.content().to_string();
    assert_eq!(dirty_content, "XYhello");

    assert!(session.key(QUIT).is_none());
    assert_eq!(
        guard_kind(&session),
        Some(&GuardKind::DirtyQuit),
        "test setup: an unpreserved dirty document must raise the quit guard"
    );

    assert!(session.key(GUARD_SAVE).is_none());
    assert!(
        session.app().guard.is_none(),
        "answering the guard always clears it"
    );
    assert!(
        session.app().quit.fan_out().is_none(),
        "a save already in flight before this press must never be enrolled \
         into the quit fan-out"
    );
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("quit cancelled \u{2014} a save was already in progress; try again once it finishes")
    );

    // The ORIGINAL ^S's ack lands now, carrying V1's version.
    assert!(session.deliver().is_none());

    assert!(
        !session.app().should_quit,
        "the stale V1 ack must never complete a quit the fan-out abandoned"
    );
    assert!(
        session.app().documents.contains_key(&id),
        "the document must still be open"
    );
    assert_eq!(
        session.app().doc(id).unwrap().buffer.content(),
        dirty_content,
        "the V2 edit, never persisted, must survive intact"
    );
    assert!(
        session.app().doc(id).unwrap().is_dirty(),
        "the V2 edit is still genuinely unsaved"
    );
}

/// `^W` `s` on a Guard whose document already has ITS OWN earlier save in
/// flight must abandon the close intent rather than close once that
/// earlier save's ack lands — the fix under test.
#[test]
fn dirty_close_never_closes_over_a_stale_save_ack() {
    let mut session = Session::open("/doc.md", "hello");
    let id = session.app().active;
    if let Some(db) = session.app_mut().db.take() {
        db.shutdown();
    }

    assert!(session.type_("X").is_none());
    assert!(session.key(SAVE).is_none());
    assert!(
        session.app().doc(id).unwrap().save_in_flight(),
        "test setup: the first ^S must have actually started a save"
    );

    assert!(session.type_("Y").is_none());
    let dirty_content = session.app().doc(id).unwrap().buffer.content().to_string();
    assert_eq!(dirty_content, "XYhello");

    assert!(session.key(CLOSE).is_none());
    assert_eq!(
        guard_kind(&session),
        Some(&GuardKind::DirtyClose),
        "test setup: a dirty document must raise the close guard"
    );

    assert!(session.key(GUARD_SAVE).is_none());
    assert!(session.app().guard.is_none(), "answering always clears it");
    assert!(
        session.app().pending_close_on_save.is_none(),
        "a save already in flight before this press must never arm a close \
         intent against it"
    );
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("close cancelled \u{2014} a save was already in progress; try again once it finishes")
    );

    // The ORIGINAL ^S's ack lands now, carrying V1's version.
    assert!(session.deliver().is_none());

    assert!(
        session.app().documents.contains_key(&id),
        "the document must still be open — the stale V1 ack must never close it"
    );
    assert_eq!(
        session.app().doc(id).unwrap().buffer.content(),
        dirty_content,
        "the V2 edit, never persisted, must survive intact"
    );
    assert!(
        session.app().doc(id).unwrap().is_dirty(),
        "the V2 edit is still genuinely unsaved"
    );
}
