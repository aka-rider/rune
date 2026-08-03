//! The tri-state resolver (`Resolution`, plan WP6.S5) and the chord-in-
//! progress state (`KeymapState`) a focused component threads a keystroke
//! through, plus the out-of-band single-key hook (`on_next_key`, WP6.S6).
//! Split out of `index.rs` to bring that file under the §1.6 500-line
//! budget; `index` re-exports every item here so no import path downstream
//! changed. This is a SEPARATE mechanism from the held-space leader
//! (`global::LEADER_BINDINGS`/`keystate::SpaceProbe`), which resolves a
//! single key confirmed by a physical hardware probe rather than a typed
//! multi-key sequence — the two do not share state and `app::handle_key`
//! consults the leader stage first (§3.4's matching order).

use crate::binding::{Binding, KeyMatch, KeyPattern};
use crate::keymap::KeyInput;
use crate::when::Context;

/// The keymap resolver's tri-state verdict on one keystroke, given the keys
/// already pending (plan WP6.S5): `None` — no binding in `table` can ever
/// match this sequence; `Pending` — at least one binding could still match
/// with more keys (carries the still-live candidates, so a which-key hint
/// can render from them); `Matched` — a complete sequence resolved to
/// exactly one command.
///
/// `Pending` owns a `Vec<Binding<C>>` rather than the `&'static [Binding<C>]`
/// the plan sketch describes: `Binding<C>` is `Copy` and every field it
/// carries is already `'static` (a `&'static str` / `&'static [KeyPattern]`),
/// so cloning the handful of still-live candidates into a small owned `Vec`
/// costs nothing at chord-table scale and needs no `Box::leak` to fake a
/// contiguous `'static` sub-slice of a table whose matching candidates were
/// never contiguous by position in the first place.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution<C: Copy + 'static> {
    None,
    Pending(Vec<Binding<C>>),
    Matched(C),
}

/// A binding whose `when` is non-empty but fails to parse is treated as
/// inactive (never matches, never keeps a sequence pending) rather than a
/// panic — CONSTITUTION §1.3. `validate` (this module's sibling) rejects a
/// malformed clause at validation time, so reaching a parse failure HERE
/// means some table's own test skipped calling it — this remains a safety
/// net for that case, not the primary enforcement. Routes through
/// `evaluate_cached` (plan WP10.S7) rather than `evaluate` so a binding's
/// clause is tokenized/parsed once for the process's lifetime, not once
/// per binding per keystroke.
fn when_holds(when: &'static str, ctx: &Context) -> bool {
    when.is_empty() || crate::when::evaluate_cached(when, ctx).unwrap_or(false)
}

/// Resolves one physical keystroke against `table`, given the sequence
/// already pending (empty if none) and the current `Context`. A binding
/// whose `when` clause doesn't hold against `ctx` is treated as absent for
/// BOTH matching and pending purposes — an inactive binding can neither
/// complete nor keep a sequence alive.
pub fn resolve<C: Copy + 'static>(
    table: &[Binding<C>],
    pending: &[KeyPattern],
    key: KeyInput,
    ctx: &Context,
) -> Resolution<C> {
    let mut typed: Vec<KeyPattern> = pending.to_vec();
    typed.push(KeyPattern {
        key: KeyMatch::Code(key.code),
        mods: key.mods,
    });

    let mut still_pending: Vec<Binding<C>> = Vec::new();
    for binding in table {
        if !when_holds(binding.when, ctx) {
            continue;
        }
        if binding.keys.len() < typed.len() {
            continue;
        }
        let prefix_matches = binding.keys.iter().zip(typed.iter()).all(|(a, b)| a == b);
        if !prefix_matches {
            continue;
        }
        if binding.keys.len() == typed.len() {
            return Resolution::Matched(binding.cmd);
        }
        still_pending.push(*binding);
    }

    if still_pending.is_empty() {
        Resolution::None
    } else {
        Resolution::Pending(still_pending)
    }
}

/// The out-of-band single-key hook (plan WP6.S6) — the one thing the
/// binding table itself cannot express: a vim-style command (e.g. `f<char>`
/// "find character") that wants the very next keystroke verbatim, bypassing
/// binding resolution entirely for exactly one key.
pub type NextKeyFn = Box<dyn FnOnce(KeyPattern) + Send>;

/// The chord-in-progress state (plan WP6.S5) — a second, general-purpose
/// pending-sequence tracker alongside `App::pending_quit` (its own bespoke
/// two-press chord) and the held-space leader's own physical-key probe
/// (`keystate::SpaceProbe`); neither of those is a typed multi-key sequence
/// through a binding table, which is what this type exists for.
#[derive(Default)]
pub struct KeymapState {
    pending: Vec<KeyPattern>,
    next_key: Option<NextKeyFn>,
}

impl KeymapState {
    pub fn pending(&self) -> &[KeyPattern] {
        &self.pending
    }

    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Registers the out-of-band hook: the very next keystroke goes to `f`
    /// instead of binding resolution.
    pub fn on_next_key(&mut self, f: NextKeyFn) {
        self.next_key = Some(f);
    }

    /// Routes a raw keystroke through the registered hook, if one is armed.
    /// Returns whether it was — the hook fires as a side effect of this
    /// call, at most once (`Option::take` clears the slot so a second
    /// keystroke never re-fires a stale hook).
    pub fn take_next_key(&mut self, key: KeyPattern) -> bool {
        match self.next_key.take() {
            Some(f) => {
                f(key);
                true
            }
            None => false,
        }
    }

    /// `Esc` (or any external cancel) clears the pending sequence and hands
    /// the consumed keys BACK, so a focused text surface can re-insert them
    /// as literal characters instead of silently losing them (plan WP6.S5:
    /// cancelling a pending sequence must hand the consumed keys back).
    pub fn cancel(&mut self) -> Vec<KeyPattern> {
        std::mem::take(&mut self.pending)
    }
}

/// Threads one keystroke through `table` and `state`'s pending sequence
/// together: on `Matched`/`None` the pending sequence is cleared (an
/// unrecognized continuation abandons the chord rather than swallowing the
/// key silently — the caller sees `None` and is free to fall through to
/// its own printable-insert path, exactly like the stateless `resolve_in`
/// today); on `Pending` it's extended by the just-typed key.
pub fn resolve_stateful<C: Copy + 'static>(
    table: &[Binding<C>],
    state: &mut KeymapState,
    key: KeyInput,
    ctx: &Context,
) -> Resolution<C> {
    let result = resolve(table, &state.pending, key, ctx);
    match &result {
        Resolution::Matched(_) | Resolution::None => state.pending.clear(),
        Resolution::Pending(_) => state.pending.push(KeyPattern {
            key: KeyMatch::Code(key.code),
            mods: key.mods,
        }),
    }
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keymap::{KeyCode, Mods};

    const CTRL: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    };

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TestCmd {
        Standalone,
        Chord,
    }

    fn key(code: KeyCode, mods: Mods) -> KeyInput {
        KeyInput { code, mods }
    }

    const fn ctrl_k() -> KeyPattern {
        KeyPattern::new(KeyCode::Char('k'), CTRL)
    }
    const fn ctrl_c() -> KeyPattern {
        KeyPattern::new(KeyCode::Char('c'), CTRL)
    }

    #[test]
    fn resolve_walks_a_chord_to_completion() {
        const TABLE: &[Binding<TestCmd>] = &[Binding {
            keys: &[ctrl_k(), ctrl_c()],
            cmd: TestCmd::Chord,
            help: "chord",
            when: "",
            alias: false,
        }];
        let ctx = Context::default();

        let first = resolve(TABLE, &[], key(KeyCode::Char('k'), CTRL), &ctx);
        assert!(matches!(first, Resolution::Pending(ref v) if v.len() == 1));

        let pending = [ctrl_k()];
        let second = resolve(TABLE, &pending, key(KeyCode::Char('c'), CTRL), &ctx);
        assert_eq!(second, Resolution::Matched(TestCmd::Chord));
    }

    #[test]
    fn resolve_returns_none_for_an_unrecognized_continuation() {
        const TABLE: &[Binding<TestCmd>] = &[Binding {
            keys: &[ctrl_k(), ctrl_c()],
            cmd: TestCmd::Chord,
            help: "chord",
            when: "",
            alias: false,
        }];
        let ctx = Context::default();
        let pending = [ctrl_k()];
        let result = resolve(TABLE, &pending, key(KeyCode::Char('q'), Mods::NONE), &ctx);
        assert_eq!(result, Resolution::None);
    }

    #[test]
    fn resolve_skips_a_binding_whose_when_clause_fails() {
        const TABLE: &[Binding<TestCmd>] = &[Binding {
            keys: &[KeyPattern::new(KeyCode::Char('k'), Mods::NONE)],
            cmd: TestCmd::Standalone,
            help: "standalone",
            when: "read_only",
            alias: false,
        }];
        let ctx = Context::default(); // read_only: false
        let result = resolve(TABLE, &[], key(KeyCode::Char('k'), Mods::NONE), &ctx);
        assert_eq!(result, Resolution::None);

        let locked = Context {
            read_only: true,
            ..Context::default()
        };
        let result = resolve(TABLE, &[], key(KeyCode::Char('k'), Mods::NONE), &locked);
        assert_eq!(result, Resolution::Matched(TestCmd::Standalone));
    }

    #[test]
    fn resolve_stateful_tracks_pending_across_calls_and_clears_on_match() {
        const TABLE: &[Binding<TestCmd>] = &[Binding {
            keys: &[ctrl_k(), ctrl_c()],
            cmd: TestCmd::Chord,
            help: "chord",
            when: "",
            alias: false,
        }];
        let ctx = Context::default();
        let mut state = KeymapState::default();

        let first = resolve_stateful(TABLE, &mut state, key(KeyCode::Char('k'), CTRL), &ctx);
        assert!(matches!(first, Resolution::Pending(_)));
        assert_eq!(state.pending(), &[ctrl_k()]);

        let second = resolve_stateful(TABLE, &mut state, key(KeyCode::Char('c'), CTRL), &ctx);
        assert_eq!(second, Resolution::Matched(TestCmd::Chord));
        assert!(!state.is_pending());
    }

    #[test]
    fn cancel_hands_back_the_consumed_keys() {
        const TABLE: &[Binding<TestCmd>] = &[Binding {
            keys: &[ctrl_k(), ctrl_c()],
            cmd: TestCmd::Chord,
            help: "chord",
            when: "",
            alias: false,
        }];
        let ctx = Context::default();
        let mut state = KeymapState::default();
        let _ = resolve_stateful(TABLE, &mut state, key(KeyCode::Char('k'), CTRL), &ctx);

        let consumed = state.cancel();
        assert_eq!(consumed, vec![ctrl_k()]);
        assert!(!state.is_pending());
    }

    #[test]
    fn on_next_key_intercepts_the_very_next_keystroke() {
        let mut state = KeymapState::default();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen_clone = seen.clone();
        state.on_next_key(Box::new(move |k| {
            *seen_clone.lock().expect("lock") = Some(k);
        }));

        let consumed = state.take_next_key(ctrl_c());
        assert!(consumed);
        assert_eq!(*seen.lock().expect("lock"), Some(ctrl_c()));

        // A second keystroke, with no hook re-armed, is not intercepted.
        assert!(!state.take_next_key(ctrl_k()));
    }
}
