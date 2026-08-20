//! Shared setup helpers for the Rename "Done when" test suite, split
//! across `rename_bind.rs` (focus/typing, the end-to-end no-store rename,
//! and draft naming), `rename_refusals.rs` (the refusal paths),
//! `rename_gate.rs` (the extension gate and the field's own word-motion/
//! selection/undo editing), `rename_clipboard.rs` (copy/cut/paste in the
//! title), `rename_collision.rs` (the collision guard and both halves of
//! hazard 1), `rename_replace.rs` (the `[R]eplace` path against a real
//! in-memory `Store`), and `rename_focus.rs` (the
//! focus-loss-is-the-commit-chokepoint suite) — this is the 500-line-budget split of
//! the original `rename.rs`, re-split once the extension-gate
//! and clipboard packages grew `rename_bind.rs` past the ceiling again.
//! Each consumer pulls this in via `mod rename_common;` — integration test
//! files are separate binaries, so this is the one place all seven draw an
//! identical fixture from, rather than risking drift.
//!
//! Two layers live here, each its own submodule (this module's own 500-line
//! budget forced the split): [`session`] is the primary one — real stores,
//! real key delivery, per-step invariant checking. [`bare_app`] is the
//! older bare-`App` layer, which survives for the consumers that cannot
//! run under the session driver yet (`reading_view.rs`,
//! `bind_new_named.rs`, `save_state_machine.rs`,
//! `materialize_dead_writer_reentrancy.rs`, `materialize_fatal_two_docs.rs`,
//! `refused_hydration_detach.rs`) and for the handful of rename tests that
//! must observe `Effects` directly (OSC52 copies, timer arming) or hold a
//! stale `Cmd` the driver's single rename slot cannot. Every item from both
//! is re-exported here so `rename_common::{...}` keeps working unchanged
//! for every consumer.
#![allow(dead_code, unused_imports)]

mod bare_app;
mod session;

pub use bare_app::*;
pub use session::*;
