//! rune-fuzz: the headless session fuzzer for the Rust port. Drives the
//! real `rune_tui::app::update` against an in-memory `Vfs` with no
//! terminal, no clock, and no subprocess, checking named invariants after
//! every settled message. Mirrors the Go tree's `internal/fuzz` split:
//! action model -> driver -> snapshot/step-context -> pure invariant
//! checkers.
//!
//! This commit lands the action/snapshot/step-context model only; `driver`,
//! `invariant`, and `generate` follow in subsequent commits (each adds its
//! own `pub mod` line here).
pub mod action;
pub mod snapshot;
pub mod step;
