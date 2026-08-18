//! One request-correlation counter shape, reused by every feature that
//! must tell a fresh async reply from a stale one (rename, merge,
//! save-confirm, quit-confirm, filesearch, search history): a `GenCounter`
//! field on `App` mints a `Generation` at request time, the request's
//! `Cmd`/`Msg` carries that `Generation` back, and the handler compares it
//! against whatever the feature's own state currently holds — a mismatch
//! means a later request already superseded this reply.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Generation(u64);

impl Generation {
    pub const ZERO: Generation = Generation(0);

    /// Builds a `Generation` from an explicit numeric value — for a fuzz
    /// driver deliberately targeting a generation `mint()` structurally
    /// cannot reach (a caller-chosen stale timer fire). Production code
    /// only ever holds a `Generation` a `GenCounter` minted.
    pub const fn from_raw(value: u64) -> Generation {
        Generation(value)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GenCounter(Generation);

impl GenCounter {
    pub fn mint(&mut self) -> Generation {
        let minted = self.0;
        self.0 = Generation(self.0.0.wrapping_add(1));
        minted
    }
}
