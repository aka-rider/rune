//! Acceptance anchor for the store-backed save pipeline: proves the real
//! driver can run an armed store-backed save all the
//! way to completion — `MaterializePrepare` ack -> the caller-side `vfs`
//! `Cmd` -> `MaterializeRecord` ack -> `save_in_flight` clearing — with no
//! step dropped on the floor and no `SAVE-INFLIGHT-SM` false positive
//! along the way.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_fuzz::driver::Session;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

fn assert_clean(result: &driver::RunResult) {
    assert!(
        result.violation.is_none(),
        "{}",
        result
            .violation
            .as_ref()
            .map(|v| format!("{}: {}", v.id, v.message))
            .unwrap_or_default()
    );
}

const SAVE_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('s'),
    mods: Mods {
        sup: true,
        ..Mods::NONE
    },
};

/// A fixed, non-random action list that arms a store-backed save and drains
/// every recovery-store round trip it takes to settle: the typed edit's own
/// `AppendEdit` ack, `⌘S`'s `MaterializePrepare` ack (spawns the caller-side
/// `vfs` `Cmd`), that `Cmd`'s own `Msg::MaterializeVfsDone` reply
/// (`Action::Deliver`), and finally the `MaterializeRecord` ack that clears
/// `save_in_flight` for good.
#[test]
fn an_armed_store_backed_save_runs_to_completion_with_no_violation() {
    let actions = vec![
        Action::Type("!".to_string()),
        Action::DeliverDb, // the typed edit's own AppendEdit ack
        Action::Key(SAVE_KEY),
        Action::DeliverDb, // MaterializePrepare ack -> spawns the vfs Cmd
        Action::Deliver,   // the vfs Cmd's own Msg::MaterializeVfsDone
        Action::DeliverDb, // MaterializeRecord ack -> save_in_flight clears
    ];
    let result = driver::run("/fuzz/doc.md", "hello", &actions);
    assert_clean(&result);
}

/// Same sequence, but delivering every recovery-store round trip through
/// `Action::DeliverDbAll` instead of one `Action::DeliverDb` per hop —
/// proves the completion path doesn't depend on the caller draining ops in
/// exactly the order this suite's other test happens to script them in.
#[test]
fn an_armed_store_backed_save_runs_to_completion_via_deliver_db_all() {
    let actions = vec![
        Action::Type("!".to_string()),
        Action::DeliverDbAll,
        Action::Key(SAVE_KEY),
        Action::DeliverDbAll,
        Action::Deliver,
        Action::DeliverDbAll,
    ];
    let result = driver::run("/fuzz/doc.md", "hello", &actions);
    assert_clean(&result);
}

/// Issue-115-follow-up B2: `state.saves_delivered_ok` (the counter
/// `SAVE-CLEAN-MATCHES-DISK`/`SAVE-NO-TRAILING-WS` gate on) actually
/// increments once a real store-backed save runs to completion through the
/// real driver — not just by construction in a hand-built `StepCtx`.
#[test]
fn a_completed_store_backed_save_is_counted_in_saves_delivered_ok() {
    let mut session = Session::open("/fuzz/doc.md", "hello");
    assert_eq!(session.type_("!"), None);
    assert_eq!(session.deliver_db(), None);
    assert_eq!(session.key(SAVE_KEY), None);
    assert_eq!(session.deliver_db(), None);
    assert_eq!(session.deliver(), None);
    assert_eq!(session.deliver_db(), None);

    assert!(
        session.saves_delivered_ok() > 0,
        "a completed store-backed save must be counted"
    );

    let result = session.finish();
    assert_clean(&result);
}

const CTRL_M: KeyInput = KeyInput {
    code: KeyCode::Char('m'),
    mods: Mods {
        ctrl: true,
        ..Mods::NONE
    },
};

const SHIFT_SUP_Y: KeyInput = KeyInput {
    code: KeyCode::Char('y'),
    mods: Mods {
        shift: true,
        sup: true,
        ..Mods::NONE
    },
};

const UP: KeyInput = KeyInput {
    code: KeyCode::Up,
    mods: Mods::NONE,
};

const SPACE: KeyInput = KeyInput {
    code: KeyCode::Char(' '),
    mods: Mods::NONE,
};

const ESCAPE: KeyInput = KeyInput {
    code: KeyCode::Escape,
    mods: Mods::NONE,
};

/// Issue-115-follow-up B2: `Action::DivergeDisk` sets
/// `disk_diverged_since_publish`, and it is genuinely possible for a LATER
/// completed seed-doc save to clear it again — this is the exact scripted
/// scenario `save-clean-matches-disk-bind-new-01.rune` pins (an ordinary
/// `⌘S` while genuinely diverged is refused locally, never spawning a
/// `vfs` `Cmd` at all; only a save that lands AFTER the divergence is
/// resolved — here, via the merge take-disk verb re-binding the document
/// — actually commits and clears the flag).
#[test]
fn diverge_disk_sets_the_flag_and_a_later_completed_seed_save_clears_it() {
    let mut session = Session::open("/fuzz/doc.md", "");
    assert_eq!(session.key(UP), None);
    assert_eq!(session.type_("hello world"), None);
    assert_eq!(session.type_("\n\n\n"), None);

    assert_eq!(session.act(Action::DivergeDisk), None);
    assert!(
        session.disk_diverged_since_publish(),
        "DivergeDisk must set disk_diverged_since_publish"
    );

    assert_eq!(session.act(Action::DeliverDbAll), None);
    assert_eq!(session.key(CTRL_M), None);
    assert_eq!(session.act(Action::DeliverDbAll), None);
    assert_eq!(session.key(SHIFT_SUP_Y), None);
    assert_eq!(session.key(CTRL_M), None);
    assert_eq!(session.type_(""), None);
    assert_eq!(session.key(SAVE_KEY), None);
    assert_eq!(session.key(SPACE), None);
    assert_eq!(session.key(ESCAPE), None);
    assert_eq!(session.key(SPACE), None);
    assert_eq!(session.act(Action::DeliverDbAll), None);
    assert_eq!(session.deliver(), None);
    assert_eq!(session.act(Action::DeliverDbAll), None);

    assert!(
        !session.disk_diverged_since_publish(),
        "a later completed seed-doc save must clear disk_diverged_since_publish"
    );
    assert!(
        session.saves_delivered_ok() > 0,
        "the completed save must be counted"
    );

    let result = session.finish();
    assert_clean(&result);
}
