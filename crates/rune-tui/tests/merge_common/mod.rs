//! Shared setup helpers for the merge-mode integration test suite, split
//! across `merge_entry.rs` (WP3), `merge_resolver.rs` (WP4), and
//! `merge_resync_guard.rs` (WP6) — review fix F9's dedupe of what used to be
//! three verbatim copies of the same key-press/op-drain/external-write
//! plumbing, following the `db_wiring_common` pattern every consumer pulls
//! its `App`/`Store` fixture from already. Each consumer pulls this in via
//! `mod merge_common;`.
#![allow(dead_code)]

#[path = "../db_wiring_common/mod.rs"]
mod db_wiring_common;

use std::path::Path;

use rune_db::DbEvent;
use rune_tui::app::{self, App};
use rune_tui::db::DbBridge;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::workspace;
use rune_vfs::Vfs;

#[allow(unused_imports)]
pub use db_wiring_common::{app_with_store, publish, recv_ok};

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

pub fn press_key(app: &mut App, key: KeyInput) -> Effects {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(key), &mut effects);
    effects
}

/// Drains the single op currently recorded in `app.db_ops` for `doc`,
/// feeding its ack through `app::update` exactly as the real runtime loop
/// would when the op's `DbEvent` arrives on `Msg::Db`.
pub fn drain_one_op_for(app: &mut App, bridge: &DbBridge, doc: DocumentId) -> Effects {
    let op_id = *app
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == doc)
        .expect("one op recorded for this document")
        .0;
    let result = recv_ok(bridge, op_id);
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Db(DbEvent::Ok { id: op_id, result }),
        &mut effects,
    );
    effects
}

pub fn drain_all_ops_for(app: &mut App, bridge: &DbBridge, doc: DocumentId) {
    while app.db_ops.iter().any(|(_, pending)| pending.doc == doc) {
        drain_one_op_for(app, bridge, doc);
    }
}

/// Re-probes `doc` by switching away to `away` and back, then drains the
/// probe's ack — the only detection wiring this feature has (no file
/// watcher). Callers must have drained every other pending op for `doc`
/// first, or `drain_one_op_for` cannot tell which entry is the probe.
pub fn reprobe(app: &mut App, bridge: &DbBridge, away: DocumentId, doc: DocumentId) {
    workspace::switch_to(app, away);
    workspace::switch_to(app, doc);
    drain_one_op_for(app, bridge, doc);
}

/// The store-backed materialize dance's middle+end hops: drains the
/// `MaterializePrepare` ack (which spawns the caller-side vfs `Cmd`), runs
/// that `Cmd`, feeds its `MaterializeVfsDone` reply back through `update`,
/// then drains whatever op that reply itself enqueued (the record ack, and
/// on a lost-bookkeeping re-baseline, a further `Load`). Callers must have
/// drained every other pending op for `doc` first, same as `reprobe`. Ends
/// with `app.db_ops` empty for `doc` whether the save committed or
/// CAS-refused into the disk-conflict Guard — assertions on which of the
/// two happened belong to the caller. Shared by [`save_and_ack`] (after its
/// own `⌘S` keypress) and the disk-conflict Guard's `[S]ave anyway` answer
/// (after its own keypress).
pub fn drain_materialize_round_trip(app: &mut App, bridge: &DbBridge, doc: DocumentId) {
    let prepare_effects = drain_one_op_for(app, bridge, doc);
    let save_cmd = prepare_effects
        .cmds
        .into_iter()
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done_msg = save_cmd.run().expect("the vfs Cmd must reply");
    let mut effects = Effects::default();
    app::update(app, vfs_done_msg, &mut effects);
    drain_all_ops_for(app, bridge, doc);
}

/// Drives a real `⌘S` all the way through [`drain_materialize_round_trip`].
pub fn save_and_ack(app: &mut App, bridge: &DbBridge, doc: DocumentId) {
    press_key(app, sup('s'));
    drain_materialize_round_trip(app, bridge, doc);
}

/// Overwrites `/doc.md`'s content in place, simulating an external editor.
pub fn external_write(vfs: &dyn Vfs, bytes: &[u8]) {
    let path = Path::new("/doc.md");
    vfs.remove(path).expect("remove the stale file");
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}
