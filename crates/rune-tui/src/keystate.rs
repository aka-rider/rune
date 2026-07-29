//! The physical-keyboard-state seam behind the held-space leader.
//!
//! A held-space leader looks like it needs key *release* events. It must not:
//! turning on the Kitty `REPORT_ALL_KEYS_AS_ESCAPE_CODES` flag that would
//! deliver them makes the terminal stop sending text bytes, and termina 0.3.3
//! never parses the associated-text section — dead-key compose and IME/CJK
//! commits would lose their only delivery channel, violating §1.4.5.
//!
//! So instead of listening for a release, we *ask* — synchronously, at the
//! instant a chord key arrives — whether the spacebar is physically down.
//! macOS answers that with `CGEventSourceKeyState`, which needs no
//! entitlement and no TCC permission (unlike `CGEventTapCreate`, whose
//! header carries explicit Accessibility language). `_CGEventSourceKeyState`
//! is exported from the top-level block of `CoreGraphics.tbd`, so a bare
//! `#[link(framework)]` links it with zero new Cargo dependencies.
//!
//! `leader_available` guards that query. `CGEventSourceKeyState` returns a
//! bare `bool` with no error channel, so it cannot report "this process has no
//! window-server session" — and measurement showed it does not fail fast in
//! that case, it *blocks* (observed: never returned in 45s with the
//! window-server mach lookup denied, versus ~17ns steady-state in a normal
//! session). A stalled query on the keystroke path would hang the editor while
//! holding an unsaved buffer, so we never reach it without first confirming a
//! session exists: `CGSessionCopyCurrentDictionary` returns NULL exactly when
//! the window server is unreachable (verified both ways on one machine — NULL
//! under a sandbox denying the mach lookup, non-NULL in the ordinary GUI
//! session). The answer is cached in a `OnceLock` and primed at startup by
//! `rune-cli::main`, so no keystroke ever pays for it.
//!
//! macOS-only is mandated by the repo's `CLAUDE.md`; there is deliberately no
//! portability stub here.

use std::sync::OnceLock;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;

    /// Returns a window-server session dictionary, or NULL when the caller is
    /// not running within a Quartz GUI session (`CGSession.h`: "or NULL if the
    /// caller is not running within a Quartz GUI session or the window server
    /// is disabled"). The one available answer to a question
    /// `CGEventSourceKeyState`'s bare `bool` cannot express.
    ///
    /// Typed as an opaque pointer rather than a `CFDictionaryRef` because the
    /// only thing this module asks is null-vs-not; binding CoreFoundation's
    /// type graph for a null check would be dead weight. The returned
    /// dictionary is +1 retained under the Copy rule, hence `CFRelease` below.
    fn CGSessionCopyCurrentDictionary() -> *const core::ffi::c_void;
}

// `CFRelease` lives in CoreFoundation, not CoreGraphics — declaring it in the
// block above links against the wrong framework and fails with an
// "undefined symbol" at link time, not at compile time.
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const core::ffi::c_void);
}

/// `kCGEventSourceStateHIDSystemState` — the HID system's live key/button
/// state, as opposed to the state at the time the current event tap
/// callback fired (`CGEventTypes.h`). Its numeric value is stable across
/// SDK versions (part of the public C enum's ABI).
const HID_SYSTEM_STATE: i32 = 1;
/// `kVK_Space` — the physical spacebar's virtual keycode (`HIToolbox
/// Events.h`). Stable across SDK versions for the same reason.
const VK_SPACE: u16 = 0x31;

/// Answers "is the spacebar physically down right now?".
///
/// A trait rather than a bare function so `App` can carry the answer source
/// as a field: production installs the real query, tests install a fixed
/// answer, and the fuzzer keeps the inert default and so stays deterministic.
/// Deliberately no `Send`/`Sync` bound — `App` is never sent across threads.
pub trait SpaceProbe {
    fn space_is_down(&self) -> bool;
}

/// The default on every `App`: the leader never fires. Keeps the fuzzer
/// deterministic and keeps tests off real hardware unless they opt in.
pub struct NullProbe;

impl SpaceProbe for NullProbe {
    fn space_is_down(&self) -> bool {
        false
    }
}

/// The real query. Installed only by `rune-cli::main`.
///
/// This reads a GLOBAL hardware table — it reports the spacebar held in any
/// application, even when the terminal is unfocused. Consult it only as
/// confirmation of a hypothesis the key handler has already formed (i.e.
/// only once an `x`/`e`/`t` press has arrived), never standalone.
pub struct HidSpaceProbe;

impl SpaceProbe for HidSpaceProbe {
    fn space_is_down(&self) -> bool {
        leader_available() && unsafe { CGEventSourceKeyState(HID_SYSTEM_STATE, VK_SPACE) }
    }
}

/// Test double — `pub` (not `#[cfg(test)]`) so integration tests in
/// `tests/`, a separate crate, can construct it.
pub struct FixedSpaceProbe(pub bool);

impl SpaceProbe for FixedSpaceProbe {
    fn space_is_down(&self) -> bool {
        self.0
    }
}

/// Whether the held-space leader can be driven by the real hardware query.
///
/// Answers once, ever, and caches it. `false` means no window-server session,
/// so `HidSpaceProbe` reports the spacebar up forever after and the leader is
/// simply inert — the `^B`/`^E`/`^T` chords still reach every pane, which is
/// exactly why they stay bound.
///
/// The session check comes FIRST and is load-bearing: with no window server,
/// `CGEventSourceKeyState` blocks rather than failing fast (see the module
/// docs), so asking it at all is the hazard. Short-circuit `&&` means a NULL
/// session dictionary returns before that call is ever reached.
///
/// Call this once at startup (`rune-cli::main` does) so the answer is already
/// cached before the first keystroke — already off the keystroke path by
/// construction, since the `OnceLock` only ever runs the real probe once.
///
/// ACCEPTED RISK (`CODE-REVIEW.md` rune-tui B finding 7, plan WP10.S8): this
/// caches window-server reachability for the whole process lifetime, on the
/// assumption that reachability cannot change mid-session (e.g. a window
/// server that becomes available after a sandboxed/headless launch). No
/// transition has ever been demonstrated on this platform, and there is no
/// notification this module could subscribe to for one — a periodic re-probe
/// would itself reintroduce the very hazard this caching exists to avoid
/// (the query blocks ~45s with no window server, so polling it on any
/// schedule risks doing that blocking wait again). `leader_available_caches_
/// its_answer_across_repeated_calls` below is the tested claim: the constancy
/// assumption is documented and pinned, not silently relied upon.
pub fn leader_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| window_server_session_exists() && probe_key_state_once())
}

/// `true` when this process can reach the window server. See the module docs
/// for why this gates the key-state query instead of merely reporting on it.
fn window_server_session_exists() -> bool {
    let session = unsafe { CGSessionCopyCurrentDictionary() };
    if session.is_null() {
        return false;
    }
    // `CGSessionCopyCurrentDictionary` follows the Core Foundation Copy rule:
    // the caller owns a +1 reference. Nothing here reads the dictionary — only
    // its nullness — so release it immediately rather than leaking one
    // CFDictionary per process.
    unsafe { CFRelease(session) };
    true
}

/// The one-and-only `CGEventSourceKeyState` call made for availability. Its
/// answer (is space down *right now*, at startup) is meaningless and
/// deliberately discarded; reaching this point at all is what `true` records.
fn probe_key_state_once() -> bool {
    let _ = unsafe { CGEventSourceKeyState(HID_SYSTEM_STATE, VK_SPACE) };
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The tested half of `leader_available`'s ACCEPTED constancy
    /// assumption (see its doc comment): whatever the real hardware probe
    /// answers on this machine, repeated calls within one process must
    /// keep returning that SAME answer — proving the `OnceLock` really
    /// does keep the query off the keystroke path, rather than merely
    /// documenting an intention nothing pins.
    #[test]
    fn leader_available_caches_its_answer_across_repeated_calls() {
        let first = leader_available();
        for _ in 0..1000 {
            assert_eq!(
                leader_available(),
                first,
                "a cached OnceLock answer must never change within one process"
            );
        }
    }

    #[test]
    fn fixed_space_probe_reports_exactly_what_it_was_constructed_with() {
        assert!(FixedSpaceProbe(true).space_is_down());
        assert!(!FixedSpaceProbe(false).space_is_down());
    }

    #[test]
    fn null_probe_never_reports_the_leader_as_down() {
        assert!(!NullProbe.space_is_down());
    }
}
