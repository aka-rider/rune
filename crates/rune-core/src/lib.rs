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

/// A live read whose length is empty or less than half of `before_len` is
/// not a legitimate shrink — it is the destructive-async-reset pattern (a
/// watcher/IME/dictation reset, or a disk read caught mid-external-rewrite)
/// until proven otherwise, and trusting it silently would discard content
/// the user can still see. `before_len` of zero has nothing to protect, so
/// it never trips this. The one shrink-suspicion chokepoint shared by every
/// caller that compares a fresh read against a trusted prior length,
/// whether the lengths are bytes (a disk read against its observation
/// history) or UTF-8 string bytes (a recovered draft against disk content).
pub fn is_suspicious_shrink(before_len: usize, after_len: usize) -> bool {
    before_len != 0 && after_len * 2 < before_len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_before_never_trips_the_shrink_guard() {
        assert!(!is_suspicious_shrink(0, 0));
    }

    #[test]
    fn under_half_is_suspicious() {
        assert!(is_suspicious_shrink(10, 4));
    }

    #[test]
    fn exactly_half_or_more_survives() {
        assert!(!is_suspicious_shrink(10, 5));
        assert!(!is_suspicious_shrink(10, 6));
    }
}
