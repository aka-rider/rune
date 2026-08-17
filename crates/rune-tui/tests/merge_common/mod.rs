//! Shared helpers for the merge-mode / save-truth integration suites, driven
//! through `rune_fuzz::Session` (the same ~39-invariant checked driver the
//! fuzzer runs): key builders, the away-and-back reprobe, and the
//! store-backed materialize dance as checked steps. Real-store construction
//! stays in `db_wiring_common`. Each consumer pulls this in via
//! `mod merge_common;`.
#![allow(dead_code)]

#[path = "../db_wiring_common/mod.rs"]
pub mod db_wiring_common;

use std::sync::Arc;

use rune_fuzz::Session;
use rune_fuzz::driver::wait_for_db_op;
use rune_tui::app::{self, App};
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::workspace;
use rune_vfs::Vfs;

pub fn bare(code: KeyCode) -> KeyInput {
    KeyInput {
        code,
        mods: Mods::NONE,
    }
}

pub fn ch(c: char) -> KeyInput {
    bare(KeyCode::Char(c))
}

pub fn sup(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    }
}

pub fn ctrl(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    }
}

pub fn chord(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

pub fn sup_shift(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods {
            shift: true,
            sup: true,
            ..Mods::NONE
        },
    }
}

pub fn take_theirs() -> KeyInput {
    sup_shift('y')
}

pub fn take_ours() -> KeyInput {
    sup_shift('u')
}

pub fn next_hunk() -> KeyInput {
    sup_shift('j')
}

pub fn prev_hunk() -> KeyInput {
    ctrl('k')
}

/// The untitled draft `Session::open` leaves open alongside the seeded
/// document — the switch-away target a reprobe needs.
pub fn untitled_draft(app: &App, seed: DocumentId) -> DocumentId {
    app.documents
        .iter()
        .map(|(&id, _)| id)
        .find(|&id| id != seed)
        .expect("the untitled draft stays open alongside the seed")
}

/// Re-probes `doc` by switching away to `away` and back, then delivers the
/// probe's ack as a checked step — the only detection wiring this feature
/// has (no file watcher). Callers must have drained every other pending op
/// first, or the oldest-first `deliver_db` would deliver that op instead of
/// the probe.
pub fn reprobe(session: &mut Session, away: DocumentId, doc: DocumentId) {
    workspace::switch_to(session.app_mut(), away);
    workspace::switch_to(session.app_mut(), doc);
    assert!(session.deliver_db().is_none());
}

/// The store-backed materialize dance's middle+end hops as checked steps:
/// the `MaterializePrepare` ack (whose caller-side vfs `Cmd` the driver
/// parks as its pending save), the `Cmd` itself, then whatever ops its
/// reply enqueued (the record ack, and on a lost-bookkeeping re-baseline, a
/// further `Load`). Callers must have drained every other pending op first,
/// same as `reprobe`. Ends with `app.db_ops` empty whether the save
/// committed or CAS-refused into the disk-conflict Guard — assertions on
/// which of the two happened belong to the caller.
pub fn drain_materialize_round_trip(session: &mut Session) {
    assert!(session.deliver_db().is_none());
    assert!(session.deliver().is_none());
    assert!(session.deliver_db_all().is_none());
}

/// Drives a real `⌘S` all the way through [`drain_materialize_round_trip`].
pub fn save_and_ack(session: &mut Session) {
    assert!(session.key(sup('s')).is_none());
    drain_materialize_round_trip(session);
}

/// Drives a real `⌘S` up to the prepare ack the pre-publish divergence gate
/// answers — no caller-side vfs `Cmd` follows a refusal, so there is nothing
/// further to discharge. What the user then sees belongs to the caller to
/// assert.
pub fn save_expecting_refusal(session: &mut Session) {
    assert!(session.key(sup('s')).is_none());
    assert!(session.deliver_db().is_none());
}

/// Delivers exactly the op named by `op_id` straight through `update`,
/// outside the checked-step cycle — for tests that deliberately hold one op
/// back to control arrival order, which the driver's oldest-first
/// `deliver_db` cannot express.
pub fn deliver_op_unchecked(session: &mut Session, op_id: u64) -> Effects {
    let bridge = Arc::clone(&session.app().db.as_ref().expect("store wired").bridge);
    let evt = wait_for_db_op(&bridge, op_id);
    let mut effects = Effects::default();
    app::update(session.app_mut(), Msg::Db(evt), &mut effects);
    effects
}

fn oldest_op_for(app: &App, doc: DocumentId) -> Option<u64> {
    app.db_ops
        .iter()
        .filter(|(_, pending)| pending.doc == doc)
        .map(|(&id, _)| id)
        .min()
}

/// [`drain_materialize_round_trip`] outside the checked-step cycle — for
/// the one fixture shape the driver's redivergence tracker cannot be told
/// the truth about: an external write landing AFTER a completed merge.
/// `Action::DivergeDisk` owns its own bytes and its own reprobe, and the
/// tracker's `note_external_write` is driver-private, so a checked delivery
/// of the save's truthful `Diverged` refusal would read as the
/// re-merge-prompt loop.
pub fn drain_materialize_round_trip_unchecked(session: &mut Session, doc: DocumentId) {
    let prepare_op = oldest_op_for(session.app(), doc).expect("prepare op enqueued");
    let prepare_effects = deliver_op_unchecked(session, prepare_op);
    let save_cmd = prepare_effects
        .cmds
        .into_iter()
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done_msg = save_cmd.run().expect("the vfs Cmd must reply");
    let mut effects = Effects::default();
    app::update(session.app_mut(), vfs_done_msg, &mut effects);
    while let Some(op_id) = oldest_op_for(session.app(), doc) {
        deliver_op_unchecked(session, op_id);
    }
}

/// Overwrites `/doc.md`'s content in place, simulating an external editor.
pub fn external_write(vfs: &dyn Vfs, bytes: &[u8]) {
    let path = std::path::Path::new("/doc.md");
    vfs.remove(path).expect("remove the stale file");
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}
