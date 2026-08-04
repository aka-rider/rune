//! The `cluster_*` strategy functions and the weighted table over them,
//! split out of `generate` (§1.6 budget) — every one of these draws its
//! fixed data from `palette.rs`.

use std::path::PathBuf;

use proptest::prelude::*;
use proptest::sample::select;

use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

use crate::action::{Action, HighlightVersion};

use super::palette::{
    ADD_CURSOR_ABOVE_KEY, ADD_CURSOR_BELOW_KEY, COPY_KEY, CTRL_B_KEY, CTRL_C_KEY, CTRL_E_KEY,
    CTRL_P_KEY, CTRL_R_KEY, CTRL_T_KEY, CUT_KEY, DELETE_KEYS, ENTER_KEY, ESCAPE_KEY,
    EXPLORER_SEARCH_KEYS, MARKDOWN_FRAGMENTS, NAV_KEYS, PASTE_KEY, PASTE_PALETTE, REDO_KEY,
    SAVE_KEY, SELECT_ALL_KEY, SELECT_MOTION_KEYS, TITLE_MOTION_KEYS, TYPE_PALETTE, UNDO_KEY,
};

fn arb_resize() -> impl Strategy<Value = (u16, u16)> {
    (1u16..=200, 2u16..=60)
}

/// Every one of the 16 `KeyCode` variants; `Char` draws an arbitrary
/// `char`. 16 arms exceeds `prop_oneof!`'s 10-arm threshold (G16), so every
/// arm is `.boxed()`. `F1` (`GlobalCommand::Help`) was the one omission
/// (CODE-REVIEW.md rune-fuzz finding 9: a stale "15 variants" doc comment
/// was true of the arms below but false of the enum, hiding that Help was
/// structurally unreachable through this generator) — now included.
fn arb_any_keycode() -> impl Strategy<Value = KeyCode> {
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
fn arb_mods() -> impl Strategy<Value = Mods> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(shift, alt, ctrl, sup)| Mods {
            shift,
            alt,
            ctrl,
            sup,
        },
    )
}

/// 35 — 3-in-4 typed prose (1-4 `TYPE_PALETTE` fragments joined by spaces),
/// 1-in-4 a `Paste` of a `PASTE_PALETTE` entry — the only path that can
/// insert `\r`, `\t`, or other control bytes (G3), so this is what actually
/// exercises the §1.4.5 byte-verbatim edge.
fn cluster_type_prose() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        3 => proptest::collection::vec(select(TYPE_PALETTE), 1..=4)
            .prop_map(|frags| vec![Action::Type(frags.join(" "))]),
        1 => select(PASTE_PALETTE).prop_map(|s| vec![Action::Paste(s.to_string())]),
    ]
}

/// 22 — 1-6 navigation keystrokes.
fn cluster_navigate() -> impl Strategy<Value = Vec<Action>> {
    proptest::collection::vec(select(NAV_KEYS), 1..=6)
        .prop_map(|keys| keys.into_iter().map(Action::Key).collect())
}

/// 10 — 1-5 shift-modified motions, or one `SelectAll` (`sup+a`).
fn cluster_selection() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        proptest::collection::vec(select(SELECT_MOTION_KEYS), 1..=5)
            .prop_map(|keys| keys.into_iter().map(Action::Key).collect()),
        Just(vec![Action::Key(SELECT_ALL_KEY)]),
    ]
}

/// 8 — 1-6 of {Backspace, Delete, Tab, BackTab}.
fn cluster_delete() -> impl Strategy<Value = Vec<Action>> {
    proptest::collection::vec(select(DELETE_KEYS), 1..=6)
        .prop_map(|keys| keys.into_iter().map(Action::Key).collect())
}

/// 7 — 1-4 `sup+z`, optionally then 1-3 `sup+shift+z`.
fn cluster_undo_redo() -> impl Strategy<Value = Vec<Action>> {
    (1usize..=4, proptest::option::of(1usize..=3)).prop_map(|(undo_n, redo_n)| {
        let mut actions = vec![Action::Key(UNDO_KEY); undo_n];
        if let Some(n) = redo_n {
            actions.extend(std::iter::repeat_n(Action::Key(REDO_KEY), n));
        }
        actions
    })
}

/// 6 — a structural markdown fragment, or the two-action code-fence form
/// (`Type("```rust")` then `Key(Enter)` — a bare `"\n"` inside a `Type`
/// payload is legal, but the fence reads more clearly as two actions).
fn cluster_markdown_write() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        7 => select(MARKDOWN_FRAGMENTS).prop_map(|s| vec![Action::Type(s.to_string())]),
        1 => Just(vec![Action::Type("```rust".to_string()), Action::Key(ENTER_KEY)]),
    ]
}

/// 5 — `Key(sup+s)`, then 3-in-4 `Deliver`.
fn cluster_save() -> impl Strategy<Value = Vec<Action>> {
    proptest::bool::weighted(0.75).prop_map(|deliver| {
        let mut actions = vec![Action::Key(SAVE_KEY)];
        if deliver {
            actions.push(Action::Deliver);
        }
        actions
    })
}

/// 4 — one of `sup+c`, `sup+x`, or (`sup+v` then a `ClipboardReply` of a
/// `PASTE_PALETTE` entry).
fn cluster_clipboard() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        Just(vec![Action::Key(COPY_KEY)]),
        Just(vec![Action::Key(CUT_KEY)]),
        select(PASTE_PALETTE).prop_map(|s| vec![
            Action::Key(PASTE_KEY),
            Action::ClipboardReply(s.to_string())
        ]),
        // Selects 1-3 chars first, THEN pastes over that selection —
        // CODE-REVIEW.md rune-fuzz finding 12: `PASTE-VERBATIM` used to
        // skip selections entirely, so a paste-over-selection (the
        // byte-displacing path §1.4.10 governs) had no fuzz-time guard
        // while the byte-safe collapsed-caret half did. Relying on
        // `cluster_selection` happening to run immediately before this one
        // would leave it to chance; this arm is self-contained.
        (1u8..=3, select(PASTE_PALETTE)).prop_map(|(n, s)| {
            let mut actions: Vec<Action> = std::iter::repeat_n(
                Action::Key(KeyInput {
                    code: KeyCode::Right,
                    mods: Mods {
                        shift: true,
                        ..Mods::NONE
                    },
                }),
                n as usize,
            )
            .collect();
            actions.push(Action::Key(PASTE_KEY));
            actions.push(Action::ClipboardReply(s.to_string()));
            actions
        }),
    ]
}

/// 3 — 3-12 arbitrary `KeyInput`s: any of the 15 `KeyCode`s x any of the 16
/// `Mods` combinations.
fn cluster_monkey_burst() -> impl Strategy<Value = Vec<Action>> {
    proptest::collection::vec(
        (arb_any_keycode(), arb_mods()).prop_map(|(code, mods)| KeyInput { code, mods }),
        3..=12,
    )
    .prop_map(|keys| keys.into_iter().map(Action::Key).collect())
}

/// 2 — a single `Deliver`.
fn cluster_async_deliver() -> impl Strategy<Value = Vec<Action>> {
    Just(vec![Action::Deliver])
}

/// An arbitrary `DirEntry`: a short ASCII name (bounded so proptest doesn't
/// waste its shrink budget on absurdly long ones) plus an arbitrary
/// `is_dir`.
fn arb_dir_entry() -> impl Strategy<Value = DirEntry> {
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

fn arb_dir_cause() -> impl Strategy<Value = DirCause> {
    prop_oneof![Just(DirCause::Nav), Just(DirCause::Refresh)]
}

/// A `DirLoaded` generation: a small bounded range, not `any::<u32>()` —
/// `Explorer::request_generation` starts at 0 and increments by 1 per
/// issued `ReadDir`, so a narrow range gives a real chance of landing
/// exactly on the live value (exercising the "applied" path) while still
/// mostly missing it (exercising the "ignored as stale" path the review fix
/// added `handle_dir_loaded`'s guard for) — deliberately NOT pinned to the
/// live generation the way `ConfirmTimeout` (G15) is.
fn arb_dir_loaded_generation() -> impl Strategy<Value = u32> {
    0u32..=4u32
}

fn arb_highlight_version() -> impl Strategy<Value = HighlightVersion> {
    prop_oneof![
        Just(HighlightVersion::Live),
        Just(HighlightVersion::Stale),
        Just(HighlightVersion::Future),
    ]
}

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
fn arb_highlight_span() -> impl Strategy<Value = (usize, usize, u16)> {
    /// Arm 1 — a small well-formed span: `start` fits inside every `SEEDS`
    /// document, `len >= 1` keeps `start < end`.
    const IN_BOUNDS_START: std::ops::Range<usize> = 0..30;
    const IN_BOUNDS_LEN: std::ops::Range<usize> = 1..15;

    /// Arm 2 — a span entirely past the end of every `SEEDS` document (the
    /// longest seed is well under 900 bytes), exercising the out-of-bounds
    /// clamp/discard path with a WIDE span, not just a narrow overrun.
    const FAR_OUT_OF_BOUNDS_START: std::ops::Range<usize> = 900..2000;
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

/// 3 — one guaranteed edit (`Key('h')`, so the live buffer version is >= 1
/// before the reply is delivered) followed by a synthesized
/// `Msg::Highlighted` reply. The edit is mandatory: `HighlightVersion::
/// Stale` resolves via `buffer.version().saturating_sub(1)`, which at
/// version 0 is silently the SAME as `Live` — an edit first guarantees
/// `Stale` is genuinely distinct (plan WP7.S6).
///
/// The guarantee is made TRUE by construction, not by assumption:
/// `Action::Key` only reaches the buffer while `app.focus() == Pane::Editor`
/// (the ordinary four-stage key pipeline), and a preceding cluster can
/// leave focus anywhere — `cluster_chrome`'s `Key(CTRL_R_KEY)` arm parks it
/// on `Pane::Title` with no restore. So this cluster prepends the same
/// two-key focus-restoring sequence `driver/checks.rs::
/// restore_editor_focus` uses at end-of-session (`ESCAPE_KEY` to dismiss
/// any modal, then `CTRL_E_KEY`/`GlobalCommand::FocusEditor` to reclaim
/// focus regardless of which pane held it) BEFORE `Key('h')`, unconditional
/// on generator-time state (there is none to condition on — this runs
/// before any session exists). Both keys are no-ops from the editor's own
/// perspective: `Esc` with no modal up either collapses a selection or is
/// consumed by whichever pane owns focus, and `^e` is idempotent when focus
/// is already `Editor`. See `cluster_highlight_edit_survives_focus_parked_
/// off_editor` below for the regression this closes.
fn cluster_highlight() -> impl Strategy<Value = Vec<Action>> {
    (
        arb_highlight_version(),
        proptest::collection::vec(arb_highlight_span(), 0..=6),
    )
        .prop_map(|(version, spans)| {
            vec![
                Action::Key(ESCAPE_KEY),
                Action::Key(CTRL_E_KEY),
                Action::Key(KeyInput {
                    code: KeyCode::Char('h'),
                    mods: Mods::NONE,
                }),
                Action::Highlight { version, spans },
            ]
        })
}

/// 1 — one of `Resize`, `FailNextSave`, `Key(ctrl+c)`, `ConfirmTimeout`,
/// `DirLoaded` with 0-6 arbitrary entries (plan WP4.S6), the named
/// `^b`/`^t` Explorer/Tabs toggle chords (CODE-REVIEW.md rune-fuzz finding
/// 10: without these, `DirLoaded` always landed in a never-opened
/// Explorer), `^r` immediately followed by one of `TITLE_MOTION_KEYS`
/// (plan WP5.S6) — so the SAME generated cluster both parks focus on the
/// title and exercises one of its own word-motion/selection/undo bindings
/// against it, not just against the document — or `^b` immediately
/// followed by 1-3 unmodified printable letters (Explorer type-to-search,
/// `explorer_search.rs`): without this arm, a generated key aimed at the
/// Explorer while it's actually focused was reachable only through
/// `cluster_monkey_burst`'s ~0.4%-of-16-mods-per-key odds, never reliably
/// enough to prove `PANE-NO-BLEED` against the new wildcard binding row.
fn cluster_chrome() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        arb_resize().prop_map(|(w, h)| vec![Action::Resize(w, h)]),
        Just(vec![Action::FailNextSave]),
        Just(vec![Action::Key(CTRL_C_KEY)]),
        Just(vec![Action::Key(CTRL_R_KEY)]),
        Just(vec![Action::Key(CTRL_B_KEY)]),
        Just(vec![Action::Key(CTRL_T_KEY)]),
        Just(vec![Action::Key(CTRL_P_KEY)]),
        Just(vec![Action::ConfirmTimeout]),
        select(TITLE_MOTION_KEYS).prop_map(|k| vec![Action::Key(CTRL_R_KEY), Action::Key(k)]),
        proptest::collection::vec(select(EXPLORER_SEARCH_KEYS), 1..=3).prop_map(|keys| {
            let mut actions = vec![Action::Key(CTRL_B_KEY)];
            actions.extend(keys.into_iter().map(Action::Key));
            actions
        }),
        (
            proptest::collection::vec(arb_dir_entry(), 0..=6),
            arb_dir_cause(),
            arb_dir_loaded_generation()
        )
            .prop_map(|(entries, cause, generation)| vec![Action::DirLoaded {
                entries,
                cause,
                generation
            }]),
    ]
}

/// 1-3 `AddCursorAbove`/`AddCursorBelow` presses (building a real
/// multi-cursor set), then a couple of ordinary edits/motions so the new
/// cursors actually do something observable — `CUR-ORDER`/the clipboard
/// invariants only ever see a `let [cursor]` single-cursor session
/// otherwise (CODE-REVIEW.md rune-fuzz finding 11: the entire multi-cursor
/// surface was monkey-burst-only at ~0.42%/key). Uses the chord actually
/// bound in `keymap/editor_bindings.rs` (`alt+sup+Up`/`Down`, `ALT_SUP`) —
/// not literally "alt+shift" as the plan text names it, which binds
/// `CloneLineUp`/`CloneLineDown` instead.
fn cluster_multicursor() -> impl Strategy<Value = Vec<Action>> {
    (
        proptest::collection::vec(
            prop_oneof![
                Just(Action::Key(ADD_CURSOR_ABOVE_KEY)),
                Just(Action::Key(ADD_CURSOR_BELOW_KEY)),
            ],
            1..=3,
        ),
        select(TYPE_PALETTE),
    )
        .prop_map(|(cursor_adds, frag)| {
            let mut actions = cursor_adds;
            actions.push(Action::Type(frag.to_string()));
            actions.push(Action::Key(COPY_KEY));
            actions
        })
}

/// Presses `^C` (may arm the quit-confirm at whatever generation is
/// currently next, OR — on a dirty, unpreserved document, which this
/// no-`db` fuzz harness always has — raise the `DirtyQuit` Guard instead;
/// either is a legitimate, already-handled outcome), immediately answers
/// `Escape` if a Guard DID come up, then unconditionally delivers
/// `Action::StaleConfirmTimeout` for a generation guaranteed to mismatch
/// whatever (if anything) ended up armed — exercising `CONFIRM-GEN`'s
/// `!should_clear` branch (CODE-REVIEW.md rune-fuzz finding 5), which
/// `Action::ConfirmTimeout` structurally cannot reach (it always echoes the
/// LIVE armed generation).
///
/// The `Escape` answer (plan WP2) is load-bearing, not merely tidy: since
/// `banner::guard::handle_dirty_quit_key`'s `d`/`D` answer now actually
/// completes the QUIT (the pre-WP2 wedge fixed by this plan), leaving a
/// `DirtyQuit` Guard open past this cluster would let ANY later cluster's
/// ordinary typed prose supply the stray `d` that ends the session early —
/// silently truncating the long tails `UNDO-TOTAL`/`WRAP-RT`/`HL-*` depend
/// on. `Escape` is always a safe no-op when NO Guard came up (stage 2/3
/// with editor focus treats a bare `Escape` as "collapse selection"), so
/// this is unconditional rather than gated on whether `^C` actually raised
/// one. Dedicated `cluster_quit_guard` below is where `[S]ave`/`[D]iscard`
/// answers get exercised instead, self-contained so THEY can't leak either.
fn cluster_confirm_stale() -> impl Strategy<Value = Vec<Action>> {
    Just(vec![
        Action::Key(CTRL_C_KEY),
        Action::Key(ESCAPE_KEY),
        Action::StaleConfirmTimeout(u32::MAX),
    ])
}

/// A dedicated, SELF-CONTAINED quit-guard scenario (plan WP2): `^C` (which,
/// against this no-`db` fuzz harness's always-unpreserved document, raises
/// the `DirtyQuit` Guard unless a two-press quit-confirm chord was already
/// pending — either outcome is fine, the answer keys below are no-ops when
/// no Guard is up), then ONE of `Esc`/`s`/`d` to resolve it, chosen and
/// answered in the SAME cluster so no Guard survives into whatever runs
/// next — exactly the leak `cluster_confirm_stale`'s own `Escape` fix
/// guards against, but exercising the OTHER two answers `CONFIRM-GEN`'s own
/// scenario deliberately never presses. `s`/`S` may start a real (no-store
/// fallback) save `Cmd`, so this also follows up with `Action::Deliver` —
/// harmless when nothing is pending — so a save this cluster started never
/// outlives it either.
fn cluster_quit_guard() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        Just(vec![Action::Key(CTRL_C_KEY), Action::Key(ESCAPE_KEY)]),
        Just(vec![
            Action::Key(CTRL_C_KEY),
            Action::Key(KeyInput {
                code: KeyCode::Char('d'),
                mods: Mods::NONE,
            }),
        ]),
        Just(vec![
            Action::Key(CTRL_C_KEY),
            Action::Key(KeyInput {
                code: KeyCode::Char('s'),
                mods: Mods::NONE,
            }),
            Action::Deliver,
        ]),
    ]
}

/// The user-approved weighted table, now over 15 clusters (plan WP7.S6
/// added `cluster_highlight`; WP14.S1 added `cluster_confirm_stale`;
/// WP14.S3 added `cluster_multicursor`; plan WP2 added `cluster_quit_
/// guard`, the dedicated, self-contained scenario for the `DirtyQuit`
/// Guard's `[S]ave`/`[D]iscard`/`Esc` answers). All arms are `.boxed()` —
/// `prop_oneof!` with >10 arms expands to `Union::new_weighted(vec![…
/// boxed…])` (G16).
pub(super) fn arb_cluster() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        35 => cluster_type_prose().boxed(),
        22 => cluster_navigate().boxed(),
        10 => cluster_selection().boxed(),
        8 => cluster_delete().boxed(),
        7 => cluster_undo_redo().boxed(),
        6 => cluster_markdown_write().boxed(),
        5 => cluster_save().boxed(),
        4 => cluster_clipboard().boxed(),
        4 => cluster_multicursor().boxed(),
        3 => cluster_monkey_burst().boxed(),
        3 => cluster_highlight().boxed(),
        2 => cluster_async_deliver().boxed(),
        1 => cluster_chrome().boxed(),
        1 => cluster_confirm_stale().boxed(),
        1 => cluster_quit_guard().boxed(),
    ]
}

#[cfg(test)]
#[path = "cluster_tests.rs"]
mod tests;
