//! rune-core: UI-free kernel — buffer, coordinate spaces, cursor set, and
//! the in-memory undo journal. No terminal, no markdown parsing.
//!
//! Every producer-bug invariant checked anywhere in the workspace (a
//! desynced `line_starts` index, a duplicate post-edit `start` in
//! `undo::reapply`, a duplicate visible claim in `rune-md`'s emitter) is
//! gated on [`assert_invariant`], never on `cfg(debug_assertions)`: an
//! ORDINARY build — including an unoptimized debug one a developer might
//! run directly — must degrade gracefully on a producer bug, never panic
//! on a real user's document. Only a test run (or a build that explicitly
//! opts in via that crate's own `strict-invariants` feature) treats the
//! violation as fatal.

pub mod bracket;
pub mod buffer;
pub mod coords;
pub mod cursor;
pub mod undo;

/// The chokepoint every "this should never happen, but let's be sure"
/// producer-bug check in the workspace routes through instead of a bare
/// `assert!`/`debug_assert!` (both evade the workspace's panic lints).
/// A macro, not a function: it expands INTO the calling crate, so
/// `cfg!(any(test, feature = "strict-invariants"))` resolves against the
/// CALLER's own `cfg(test)`/`strict-invariants` feature — a function
/// defined here would instead compile once against rune-core's own cfg,
/// silently disarming every dependent crate's checks under that crate's
/// own `cargo test`. `$cond` sits inside the `if`, so a disarmed build
/// never evaluates it at runtime, however expensive; `$msg` stays a
/// closure so its `format!` cost is paid only when the check actually
/// fires.
#[macro_export]
macro_rules! assert_invariant {
    ($cond:expr, $msg:expr $(,)?) => {
        if cfg!(any(test, feature = "strict-invariants")) {
            assert!($cond, "{}", ($msg)());
        }
    };
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
