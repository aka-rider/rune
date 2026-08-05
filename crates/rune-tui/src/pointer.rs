//! Mouse input, decoupled from `termina` (plan WP7.S4/S5) — the same
//! pattern `keymap.rs` uses for `KeyCode`/`KeyInput`: a platform- and
//! library-independent event type, with `from_termina` the one bridge.
//!
//! Also `PointerState`, the multi-click tracker `commands::mouse` drives
//! (plan WP7.S5): 500 ms window AND Chebyshev distance <= 1 cell
//! (deliberately not pixel-exact, since a human hand drifts). The wall
//! clock never gets read directly here —
//! `Clock` is an injected field on `App`, so a click sequence is
//! reproducible from a fuzz seed instead of depending on real elapsed time.

use std::cell::Cell;
use std::time::{Duration, Instant};

/// A mouse button, decoupled from `termina::event::MouseButton`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// A mouse action, decoupled from `termina::event::MouseEventKind`. `Moved`
/// (button-up hover) and the horizontal scroll variants are dropped by
/// `from_termina` below — mode 1002 (`ButtonEventMouse`, `term.rs`) never
/// reports plain hover in the first place, and this crate has no
/// horizontal-scroll gesture to drive (horizontal scroll is explicitly out
/// of scope for this plan).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Down(MouseButton),
    Up(MouseButton),
    Drag(MouseButton),
    ScrollUp,
    ScrollDown,
}

/// One mouse event, in zero-based terminal cell coordinates (matching
/// `termina::event::MouseEvent`'s own convention) plus the modifier keys
/// held at the time (alt-click/shift-click, plan WP7.S6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseInput {
    pub kind: MouseKind,
    pub column: u16,
    pub row: u16,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Translates a termina mouse event, or `None` for a variant this crate
/// doesn't bind (`Moved`, the horizontal scroll directions).
pub fn from_termina(event: termina::event::MouseEvent) -> Option<MouseInput> {
    use termina::event::{Modifiers as TM, MouseButton as TB, MouseEventKind as TK};

    let button = |b: TB| match b {
        TB::Left => MouseButton::Left,
        TB::Right => MouseButton::Right,
        TB::Middle => MouseButton::Middle,
    };

    let kind = match event.kind {
        TK::Down(b) => MouseKind::Down(button(b)),
        TK::Up(b) => MouseKind::Up(button(b)),
        TK::Drag(b) => MouseKind::Drag(button(b)),
        TK::ScrollUp => MouseKind::ScrollUp,
        TK::ScrollDown => MouseKind::ScrollDown,
        TK::Moved | TK::ScrollLeft | TK::ScrollRight => return None,
    };

    let m = event.modifiers;
    Some(MouseInput {
        kind,
        column: event.column,
        row: event.row,
        shift: m.contains(TM::SHIFT),
        alt: m.contains(TM::ALT),
        ctrl: m.contains(TM::CONTROL),
    })
}

/// Answers "what time is it right now?" — a trait rather than a bare
/// `Instant::now()` call so `App` can carry the answer source as a field
/// (plan WP7.S5: "inject the clock as a field so the fuzzer can reproduce
/// a gesture"). Deliberately no `Send`/`Sync` bound — `App` is never sent
/// across threads.
pub trait Clock: std::fmt::Debug {
    fn now(&self) -> Instant;
}

/// The default on every `App`: the real wall clock.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Test double — `pub` (not `#[cfg(test)]`) so integration tests in
/// `tests/`, a separate crate, can construct and advance it. `Instant` has
/// no public constructor other than `now()`, so this captures one real
/// `Instant` as its epoch and
/// advances a `Duration` offset from it rather than fabricating instants
/// directly — the ABSOLUTE time is irrelevant to `PointerState`, only the
/// durations BETWEEN clicks are.
#[derive(Debug)]
pub struct ManualClock {
    epoch: Instant,
    elapsed: Cell<Duration>,
}

impl Default for ManualClock {
    fn default() -> Self {
        ManualClock::new()
    }
}

impl ManualClock {
    pub fn new() -> ManualClock {
        ManualClock {
            epoch: Instant::now(),
            elapsed: Cell::new(Duration::ZERO),
        }
    }

    /// Advances this clock by `d` — the only way its `now()` ever changes.
    pub fn advance(&self, d: Duration) {
        self.elapsed.set(self.elapsed.get() + d);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.epoch + self.elapsed.get()
    }
}

/// The multi-click threshold: 500 ms (plan WP7.S5).
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(500);

/// The multi-click distance: Chebyshev (max of the row/column deltas) <= 1
/// cell (plan WP7.S5) — a straight-line/Euclidean distance would reject a
/// click one cell diagonally off the last one, which a real hand produces
/// often enough that this deliberately doesn't use it.
const MULTI_CLICK_DIST: i32 = 1;

fn chebyshev(c1: u16, r1: u16, c2: u16, r2: u16) -> i32 {
    let dc = (i32::from(c1) - i32::from(c2)).abs();
    let dr = (i32::from(r1) - i32::from(r2)).abs();
    dc.max(dr)
}

/// Which splitter a drag is moving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Splitter {
    LeftColumn,
    ExplorerTabs,
}

/// The in-flight drag. Text selection and splitter dragging are mutually
/// exclusive by construction — one gesture owns the pointer at a time.
#[derive(Clone, Copy, Debug)]
pub enum Drag {
    /// Extending a selection from this buffer byte offset.
    Text { anchor: usize },
    /// Moving a splitter. `grab_delta` is the offset between the grabbed
    /// cell and the splitter's own edge, so the splitter never jumps to
    /// the pointer on the first drag event.
    Splitter { which: Splitter, grab_delta: i32 },
}

/// The click-aggregation + drag state a left-button gesture needs across
/// messages (plan WP7.S5): `last_click`/`click_count` decide whether THIS
/// click continues a double/triple-click run; `drag` is the in-flight
/// gesture a `Drag` event extends, `None` once the button is released.
#[derive(Debug, Default)]
pub struct PointerState {
    pub last_click: Option<(Instant, u16, u16)>,
    pub click_count: u8,
    pub drag: Option<Drag>,
}

impl PointerState {
    /// Registers a left-button press at `(col, row)` at time `now` and
    /// returns the resulting click count — `1` for a fresh click, `2`/`3`
    /// when it continues a run within `MULTI_CLICK_WINDOW` and
    /// `MULTI_CLICK_DIST` of the previous one. Caps at `3`: a fourth quick
    /// click in place is treated the same as a triple (whole-line select),
    /// the same convention most editors use rather than growing a
    /// selection unit past "logical line".
    pub fn register_click(&mut self, now: Instant, col: u16, row: u16) -> u8 {
        let continues = self.last_click.is_some_and(|(t, c, r)| {
            now.saturating_duration_since(t) <= MULTI_CLICK_WINDOW
                && chebyshev(c, r, col, row) <= MULTI_CLICK_DIST
        });
        self.click_count = if continues {
            (self.click_count + 1).min(3)
        } else {
            1
        };
        self.last_click = Some((now, col, row));
        self.click_count
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn clicks_within_the_window_and_distance_form_a_multi_click_run() {
        let clock = ManualClock::new();
        let mut pointer = PointerState::default();
        assert_eq!(pointer.register_click(clock.now(), 10, 5), 1);
        clock.advance(Duration::from_millis(400));
        assert_eq!(
            pointer.register_click(clock.now(), 10, 5),
            2,
            "400ms apart -> double-click"
        );
        clock.advance(Duration::from_millis(400));
        assert_eq!(
            pointer.register_click(clock.now(), 10, 5),
            3,
            "another quick click -> triple-click"
        );
    }

    #[test]
    fn clicks_past_the_window_reset_to_single_clicks() {
        let clock = ManualClock::new();
        let mut pointer = PointerState::default();
        assert_eq!(pointer.register_click(clock.now(), 10, 5), 1);
        clock.advance(Duration::from_millis(600));
        assert_eq!(
            pointer.register_click(clock.now(), 10, 5),
            1,
            "600ms apart -> two single clicks"
        );
    }

    #[test]
    fn clicks_more_than_one_cell_apart_reset_to_single_clicks() {
        let clock = ManualClock::new();
        let mut pointer = PointerState::default();
        assert_eq!(pointer.register_click(clock.now(), 10, 5), 1);
        clock.advance(Duration::from_millis(100));
        assert_eq!(
            pointer.register_click(clock.now(), 20, 5),
            1,
            "far away -> not a multi-click"
        );
    }

    #[test]
    fn a_diagonal_neighbor_cell_still_counts_as_the_same_click() {
        let clock = ManualClock::new();
        let mut pointer = PointerState::default();
        assert_eq!(pointer.register_click(clock.now(), 10, 5), 1);
        clock.advance(Duration::from_millis(100));
        assert_eq!(
            pointer.register_click(clock.now(), 11, 6),
            2,
            "Chebyshev distance 1 -> still a double-click"
        );
    }
}
