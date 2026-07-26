//! rune-fuzz: the headless session fuzzer for the Rust port. Drives the
//! real `rune_tui::app::update` against an in-memory `Vfs` with no
//! terminal, no clock, and no subprocess, checking named invariants after
//! every settled message. Mirrors the Go tree's `internal/fuzz` split:
//! action model -> driver -> snapshot/step-context -> pure invariant
//! checkers.
//!
//! # Invariant roster (WP3: six)
//!
//! - `NO-PANIC` — the driver caught an unwind (a `debug_assert!`
//!   tripping, or any other panic) while settling a message.
//! - `CUR-BOUNDS` — every cursor's `position`/`anchor` is a valid, in-bounds,
//!   char-boundary byte offset (§1.3, §1.5).
//! - `CUR-ORDER` — cursors are ordered and non-overlapping.
//! - `CUR-ID` — at least one cursor; every id is non-zero and unique.
//! - `BUF-LINE-INDEX` — the line index (`line_count`/`line_starts`/
//!   `line_ends`) is internally consistent with the buffer content.
//! - `VERSION-MONOTONE` — `Buffer::version()`/`saved_version` never regress
//!   across a step.
//!
//! A later work package adds 15 more of the same three checker shapes
//! (`invariant.rs`'s module docs) for a final roster of 21.
pub mod action;
pub mod driver;
pub mod generate;
pub mod invariant;
pub mod snapshot;
pub mod step;
