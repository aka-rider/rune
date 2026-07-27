//! WP6.S4/S5 detection tests for the 15 invariants added on top of WP3's
//! six (`tests/tripwire.rs`, at its own 500-line-adjacent size and owned by
//! a parallel work package, is deliberately NOT touched here — this is a
//! separate, non-`#[ignore]`d file so `make test` still runs all of it).
//!
//! Same pattern as `tests/tripwire.rs` (WP4.S3/S4): one hand-built BAD
//! input per invariant asserting the checker fires with the right id, one
//! well-formed companion of the same shape asserting `None`. Every checker
//! is called DIRECTLY, never through `invariant::check_all`, so first-wins
//! ordering can never mask a case.
//!
//! One test TARGET (`--test invariants`, matching the Done-when gate), but
//! split into small per-domain files under `tests/invariants/` — §1.6 caps
//! any one file at 500 LoC, and one flat file for all 15 invariants would
//! blow well past that.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

// `tests/invariants.rs` is itself this test binary's crate ROOT (cargo
// gives every file directly under `tests/` its own binary), so a plain
// `mod foo;` would look for a sibling `tests/foo.rs` — NOT
// `tests/invariants/foo.rs`. `#[path]` points each submodule at its real
// home under `tests/invariants/` instead.
#[path = "invariants/clipboard.rs"]
mod clipboard;
#[path = "invariants/journal.rs"]
mod journal;
#[path = "invariants/protocol.rs"]
mod protocol;
#[path = "invariants/render_cells.rs"]
mod render_cells;
#[path = "invariants/save_disk.rs"]
mod save_disk;
#[path = "invariants/support.rs"]
mod support;
#[path = "invariants/wrap_rt.rs"]
mod wrap_rt;
