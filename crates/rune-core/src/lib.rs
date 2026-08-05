//! rune-core: UI-free kernel — buffer, coordinate spaces, cursor set, and
//! the in-memory undo journal. No terminal, no markdown parsing.
//!
//! Every producer-bug invariant this crate checks (a desynced `line_starts`
//! index, a duplicate post-edit `start` in `undo::reapply`) is gated on
//! [`STRICT_INVARIANTS`], never on `cfg(debug_assertions)`: an ORDINARY
//! build — including an unoptimized debug one a developer might run
//! directly — must degrade gracefully on a producer bug, never panic on a
//! real user's document. Only a test run (or a build that explicitly opts
//! in via the `strict-invariants` feature) treats the violation as fatal.
//! Mirrors `rune-md`'s and `rune-syntax`'s own identically-named
//! chokepoint — each crate's gate governs only its own invariants.

pub mod buffer;
pub mod coords;
pub mod cursor;
pub mod undo;

/// `true` only in test builds or when the `strict-invariants` feature is
/// explicitly enabled. `cfg!()` folds this to a compile-time literal, so an
/// `if STRICT_INVARIANTS { assert!(...) }` guard compiles away entirely
/// (dead code, zero cost) in an ordinary shipped build.
pub(crate) const STRICT_INVARIANTS: bool = cfg!(any(test, feature = "strict-invariants"));

/// The chokepoint every "this should never happen, but let's be sure"
/// producer-bug check in this crate routes through — a single place that
/// decides whether a violation panics, so no call site has to repeat the
/// `if STRICT_INVARIANTS { assert!(...) }` boilerplate (or risk getting it
/// wrong). `msg` is a closure so the `format!` cost is paid only when the
/// check is actually active.
pub(crate) fn assert_invariant(cond: bool, msg: impl FnOnce() -> String) {
    if STRICT_INVARIANTS {
        assert!(cond, "{}", msg());
    }
}
