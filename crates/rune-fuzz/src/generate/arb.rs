//! Raw arbitrary-value generators, split out of `cluster.rs` (pre-existing
//! 500-line budget): every `Strategy` here draws an unstructured primitive
//! value (a keycode, a mods combination, a dir entry, a highlight span, ...)
//! with no `Action` shape of its own — the `cluster_*` strategies in
//! `cluster.rs` compose these into the `Vec<Action>` sequences a session
//! actually runs.

use std::path::PathBuf;

use proptest::prelude::*;

use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

use crate::action::HighlightVersion;

pub(super) fn arb_resize() -> impl Strategy<Value = (u16, u16)> {
    (1u16..=200, 2u16..=60)
}

/// Every one of the 16 `KeyCode` variants; `Char` draws an arbitrary
/// `char`. 16 arms exceeds `prop_oneof!`'s 10-arm threshold (G16), so every
/// arm is `.boxed()`. `F1` (`GlobalCommand::Help`) was the one omission
/// (CODE-REVIEW.md rune-fuzz finding 9: a stale "15 variants" doc comment
/// was true of the arms below but false of the enum, hiding that Help was
/// structurally unreachable through this generator) — now included.
pub(super) fn arb_any_keycode() -> impl Strategy<Value = KeyCode> {
    prop_oneof![
        any::<char>().prop_map(KeyCode::Char).boxed(),
        Just(KeyCode::Enter).boxed(),
        Just(KeyCode::Backspace).boxed(),
        Just(KeyCode::Tab).boxed(),
        Just(KeyCode::BackTab).boxed(),
        Just(KeyCode::Escape).boxed(),
        Just(KeyCode::Left).boxed(),
        Just(KeyCode::Right).boxed(),
        Just(KeyCode::Up).boxed(),
        Just(KeyCode::Down).boxed(),
        Just(KeyCode::Home).boxed(),
        Just(KeyCode::End).boxed(),
        Just(KeyCode::PageUp).boxed(),
        Just(KeyCode::PageDown).boxed(),
        Just(KeyCode::Delete).boxed(),
        Just(KeyCode::F1).boxed(),
    ]
}

/// Any of the 16 `Mods` combinations (4 independent bools).
pub(super) fn arb_mods() -> impl Strategy<Value = Mods> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(shift, alt, ctrl, sup)| Mods {
            shift,
            alt,
            ctrl,
            sup,
        },
    )
}

/// An arbitrary `DirEntry`: a short ASCII name (bounded so proptest doesn't
/// waste its shrink budget on absurdly long ones) plus an arbitrary
/// `is_dir`.
pub(super) fn arb_dir_entry() -> impl Strategy<Value = DirEntry> {
    ("[a-zA-Z0-9_.]{0,12}", any::<bool>()).prop_map(|(name, is_dir)| {
        // WP13.S1: the fuzzer's fixture names are always plain ASCII, so
        // `path` derived straight from `name` round-trips exactly — the
        // lossy-decode gap this field exists to close only ever opens on
        // real, non-UTF-8 filenames, which `rune-vfs`'s own tests cover
        // directly.
        let path = PathBuf::from(&name);
        DirEntry { name, path, is_dir }
    })
}

pub(super) fn arb_dir_cause() -> impl Strategy<Value = DirCause> {
    prop_oneof![Just(DirCause::Nav), Just(DirCause::Refresh)]
}

/// A `DirLoaded` generation: a small bounded range, not `any::<u32>()` —
/// `Explorer::request_generation` starts at 0 and increments by 1 per
/// issued `ReadDir`, so a narrow range gives a real chance of landing
/// exactly on the live value (exercising the "applied" path) while still
/// mostly missing it (exercising the "ignored as stale" path the review fix
/// added `handle_dir_loaded`'s guard for) — deliberately NOT pinned to the
/// live generation the way `ConfirmTimeout` (G15) is.
pub(super) fn arb_dir_loaded_generation() -> impl Strategy<Value = u32> {
    0u32..=4u32
}

pub(super) fn arb_highlight_version() -> impl Strategy<Value = HighlightVersion> {
    prop_oneof![
        Just(HighlightVersion::Live),
        Just(HighlightVersion::Stale),
        Just(HighlightVersion::Future),
    ]
}

/// Shared by `arb_highlight_span`'s well-formed arm and `arb_tree_base`'s
/// (`cluster.rs`) own well-formed-vs-hostile split: a small in-bounds start
/// every `SEEDS` document fits (`arb_highlight_span`) or every
/// `TREE_FIXTURES` entry fits (`arb_tree_base`).
pub(super) const IN_BOUNDS_START: std::ops::Range<usize> = 0..30;

/// Shared the same way as `IN_BOUNDS_START`: a start entirely past the end
/// of every `SEEDS`/`TREE_FIXTURES` fixture (the longest is well under 900
/// bytes), exercising the out-of-bounds clamp/discard path.
pub(super) const FAR_OUT_OF_BOUNDS_START: std::ops::Range<usize> = 900..2000;

/// One raw `(start, end, ScopeId)` triple, deliberately unvalidated —
/// `Action::Highlight`'s own docs — drawn from four shapes: a small
/// well-formed range, a range entirely past a short document's length, a
/// deliberately inverted `start > end` pair, and a narrow 1-byte-wide range
/// at a small odd offset, chosen to land mid-`char` inside a CJK seed's
/// multi-byte code points (`SEEDS`, `palette.rs`) about as often as it lands
/// on a real boundary. Every arm draws `ScopeId` from the same `SCOPE_ID`
/// range — an out-of-range scope id isn't a shape under test here, just
/// filler, so all four arms share it rather than repeating an unexplained
/// `30` four times.
///
/// Readability-only split (finding B): each bound below is the exact
/// literal the four `prop_oneof!` arms used inline before, just named —
/// the generated distribution is unchanged.
pub(super) fn arb_highlight_span() -> impl Strategy<Value = (usize, usize, u16)> {
    const IN_BOUNDS_LEN: std::ops::Range<usize> = 1..15;

    /// Arm 2 — a span entirely past the end of every `SEEDS` document (the
    /// longest seed is well under 900 bytes), exercising the out-of-bounds
    /// clamp/discard path with a WIDE span, not just a narrow overrun.
    const FAR_OUT_OF_BOUNDS_LEN: std::ops::Range<usize> = 1..200;

    /// Arm 3 — a deliberately inverted `start > end` pair: `end` first,
    /// then a positive `gap` added to it to derive `start`, so `start` is
    /// always strictly greater than `end`.
    const INVERTED_GAP: std::ops::Range<usize> = 1..30;
    const INVERTED_END: std::ops::Range<usize> = 0..30;

    /// Arm 4 — a narrow 1-byte-wide span at a small offset, landing
    /// mid-`char` inside a CJK seed's multi-byte code points about as often
    /// as it lands on a real boundary.
    const MID_CHAR_START: std::ops::Range<usize> = 0..24;

    /// Shared by every arm — filler, not itself a shape under test.
    const SCOPE_ID: std::ops::Range<u16> = 0..30;

    prop_oneof![
        (IN_BOUNDS_START, IN_BOUNDS_LEN, SCOPE_ID).prop_map(|(start, len, scope)| (
            start,
            start + len,
            scope
        )),
        (FAR_OUT_OF_BOUNDS_START, FAR_OUT_OF_BOUNDS_LEN, SCOPE_ID)
            .prop_map(|(start, len, scope)| (start, start + len, scope)),
        (INVERTED_GAP, INVERTED_END, SCOPE_ID).prop_map(|(gap, end, scope)| (
            end + gap,
            end,
            scope
        )),
        (MID_CHAR_START, SCOPE_ID).prop_map(|(start, scope)| (start, start + 1, scope)),
    ]
}
