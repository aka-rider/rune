//! Shared setup helpers for the Rename "Done when" test suite, split
//! across `rename_bind.rs` (focus/typing, the end-to-end no-store rename,
//! and draft naming), `rename_refusals.rs` (the refusal paths),
//! `rename_gate.rs` (the extension gate and the field's own word-motion/
//! selection/undo editing), `rename_clipboard.rs` (copy/cut/paste in the
//! title), `rename_collision.rs` (the collision guard and both halves of
//! hazard 1), `rename_replace.rs` (the `[R]eplace` path against a real
//! in-memory `Store`), and `rename_focus_bind.rs`/`rename_focus_close.rs`
//! (the focus-loss-is-the-commit-chokepoint suite) — this is the
//! 500-line-budget split of the original `rename.rs`, re-split once the
//! extension-gate and clipboard packages grew `rename_bind.rs` past the
//! ceiling again. Each consumer pulls this in via `mod rename_common;` —
//! integration test files are separate binaries, so this is the one place
//! every one of them draws an identical fixture from, rather than risking
//! drift. `bind_new_named.rs` (work package A's named-but-unpublished
//! shape), `reading_view.rs` (fixture reuse only), and the bare-`App`
//! consumers named below pull it in too, and so do three
//! `set_doc_db_for_test` consumers entirely outside the Rename suite:
//! `db_wiring_rebind_replica.rs`, `db_wiring_undo_rebase.rs`,
//! `undo_shared_row_drift.rs`.
//!
//! Two layers live here, each its own submodule (this module's own 500-line
//! budget forced the split): [`session`] is the primary one — real stores,
//! real key delivery, per-step invariant checking; `bind_new_named.rs` and
//! most of `rename_bind.rs`'s own store-bound draft-naming tests are driven
//! through it now that `SAVE-INFLIGHT-SM` recognizes a title-focused Enter
//! as a legitimate `bind_new_now` commit. [`bare_app`] is the older
//! bare-`App` layer, which survives for consumers that fabricate a
//! `Msg::Db`/`Msg` the real bridge could never produce on demand (a dead
//! writer's `DbEvent::Err`/`Fatal`, a stale `MaterializeVfsDone` ticket, a
//! hand-built `LoadResult` fed straight to `handle_load_ack`) —
//! `save_state_machine.rs`, `materialize_dead_writer_reentrancy.rs`,
//! `materialize_fatal_two_docs.rs`, `refused_hydration_detach.rs` — a
//! reaction to a race the deterministic harness cannot literally
//! reproduce, not something `Session`'s checked action grammar should ever
//! grow a hole for. It also survives for `reading_view.rs`'s one test that
//! pokes `save_in_flight` directly through `begin_save` (no checked step
//! runs in between, so `Session`'s own snapshot would desync), for the
//! handful of rename tests that must observe `Effects` directly (OSC52
//! copies, `rename_clipboard.rs`'s timer-arming check) or inject a
//! `PasteTarget::Title`/`PasteTarget::Document` reply `Session`'s own
//! `Action::ClipboardReply` cannot target (`rename_clipboard.rs`,
//! `rename_focus_close.rs`), hold a stale `Cmd` the driver's single rename
//! slot cannot (`rename_collision.rs`'s `a_stale_rename_reply_is_dropped`),
//! or run a `ReadDir`/`ReadFile` `Cmd` by hand, which the driver drops
//! (`rename_focus_bind.rs`) — the same reason `explorer_common::app_with`
//! stays `App`-shaped for `navhistory_common::browsing_app`. Every item
//! from both is re-exported here so `rename_common::{...}` keeps working
//! unchanged for every consumer.
#![allow(dead_code, unused_imports)]

mod bare_app;
mod session;

pub use bare_app::*;
pub use session::*;
