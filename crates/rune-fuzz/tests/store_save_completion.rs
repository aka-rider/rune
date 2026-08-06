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
