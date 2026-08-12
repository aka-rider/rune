//! The `cluster_*` strategy functions and the weighted table over them,
//! split out of `generate` (500-line budget) — every one of these
//! draws its fixed data from `palette.rs`, composing the raw value
//! generators in `arb.rs` into `Vec<Action>` sequences.

use proptest::prelude::*;
use proptest::sample::select;

use rune_tui::keymap::{KeyCode, KeyInput, Mods};

use crate::action::Action;

use super::arb::{
    FAR_OUT_OF_BOUNDS_START, IN_BOUNDS_START, arb_any_keycode, arb_dir_cause, arb_dir_entry,
    arb_dir_loaded_generation, arb_highlight_span, arb_highlight_version, arb_mods, arb_resize,
};
#[cfg(test)]
pub(super) use super::arb::{RESIZE_MIN_HEIGHT, RESIZE_MIN_WIDTH};
use super::palette::{
    ADD_CURSOR_ABOVE_KEY, ADD_CURSOR_BELOW_KEY, COPY_KEY, CTRL_B_KEY, CTRL_C_KEY, CTRL_E_KEY,
    CTRL_P_KEY, CTRL_R_KEY, CTRL_T_KEY, CUT_KEY, DELETE_KEYS, ENTER_KEY, ESCAPE_KEY,
    EXPLORER_SEARCH_KEYS, FILESEARCH_KEY_CTRL, FILESEARCH_KEY_SUP, MARKDOWN_FRAGMENTS, MERGE_KEY,
    MERGE_RESOLVE_KEYS, NAV_KEYS, PASTE_KEY, PASTE_PALETTE, REDO_KEY, SAVE_KEY, SELECT_ALL_KEY,
    SELECT_MOTION_KEYS, TITLE_MOTION_KEYS, TRASH_KEY, TYPE_PALETTE, UNDO_KEY,
};

/// 35 — 3-in-4 typed prose (1-4 `TYPE_PALETTE` fragments joined by spaces),
/// 1-in-4 a `Paste` of a `PASTE_PALETTE` entry — the only path that can
/// insert `\r`, `\t`, or other control bytes (G3), so this is what actually
/// exercises the byte-verbatim paste edge.
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
        // byte-displacing path) had no fuzz-time guard
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

/// 2 — a single `Deliver`, a `DeliverDb`/`DeliverDbAll` some of the time
/// alongside it: the store-backed session enqueues a fresh
/// recovery-store op on nearly every edit, so this cluster is also this
/// generator's main way of keeping that backlog from just growing across a
/// session — `cluster_merge` below flushes the WHOLE backlog at its own
/// checkpoints instead of relying on this one, precisely because it can't
/// assume this cluster (or any other) already kept it empty.
fn cluster_async_deliver() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        Just(vec![Action::Deliver]),
        Just(vec![Action::Deliver, Action::DeliverDb]),
        Just(vec![Action::DeliverDb]),
        Just(vec![Action::DeliverDbAll]),
    ]
}

/// 3 — one guaranteed edit (`Key('h')`, so the live buffer version is >= 1
/// before the reply is delivered) followed by a synthesized
/// `Msg::Highlighted` reply. The edit is mandatory: `HighlightVersion::
/// Stale` resolves via `buffer.version().saturating_sub(1)`, which at
/// version 0 is silently the SAME as `Live` — an edit first guarantees
/// `Stale` is genuinely distinct (plan WP7.S6).
///
/// The guarantee is made TRUE BY CONSTRUCTION regardless of which pane a
/// preceding cluster left focused: `Action::Key` only reaches the buffer
/// while `app.focus() == Pane::Editor` (the ordinary four-stage key
/// pipeline), and this cluster is generated statically, with no live
/// `app.focus()` to branch on. It prepends a `[CTRL_T_KEY, ESCAPE_KEY]` pair
/// BEFORE `Key('h')`, unconditional on generator-time state. `^T`
/// (`GlobalCommand::FocusTabs`) is resolved in the dispatch pipeline's
/// global stage, ahead of any pane's own keymap, so it fires no matter
/// which pane owns focus and is never consumed as ordinary text (in
/// particular it survives Explorer live-search, where a plain `Escape`
/// would only clear the query); its handler unconditionally reveals the
/// left column and focuses Tabs. From Tabs, `Escape` unconditionally
/// returns focus to the Editor. So the pair is a state-free focus reset,
/// proven from Editor, Title, Explorer, Tabs, Explorer live-search,
/// Messages, and mid-quit-confirm starting focus.
///
/// One caveat survives, and it is narrower than it first looks: while a
/// Guard capture is up it swallows every key, including `^T`, so `^T` alone
/// cannot reach the Editor through one. But `ESCAPE_KEY` is the Guard's own
/// cancel key (`guard::handle_guard_key`) — it clears the Guard without
/// completing whatever the Guard was blocking, handing focus back to
/// wherever it already was. So the SAME `[CTRL_T_KEY, ESCAPE_KEY]` pair
/// still recovers even behind a Guard: `^T` is swallowed and wasted, but
/// the trailing `Escape` answers the Guard instead of colliding with
/// nothing, and the reset survives. `cluster_chrome`'s bare `^C` on a dirty,
/// unpreserved document (this harness's only kind) is exactly this case: it
/// raises the `DirtyQuit` Guard rather than leaving focus untouched on the
/// Editor, and the prefix still lands its edit behind it (see the
/// `"'x' edit, ^C (DirtyQuit Guard up)"` case below).
///
/// See `cluster_highlight_edit_survives_focus_parked_off_editor` below for
/// the regression this prefix closes.
fn cluster_highlight() -> impl Strategy<Value = Vec<Action>> {
    (
        arb_highlight_version(),
        proptest::collection::vec(arb_highlight_span(), 0..=6),
    )
        .prop_map(|(version, spans)| {
            vec![
                Action::Key(CTRL_T_KEY),
                Action::Key(ESCAPE_KEY),
                Action::Key(KeyInput {
                    code: KeyCode::Char('h'),
                    mods: Mods::NONE,
                }),
                Action::Highlight { version, spans },
            ]
        })
}

/// `base` for `Action::HighlightTree`: a small in-bounds anchor (every
/// `TREE_FIXTURES` entry is well under 30 bytes) or a deliberately far
/// out-of-bounds one — the same hostile-vs-well-formed split
/// `arb_highlight_span`'s own two bound arms use, since the render query's
/// clamp against an out-of-bounds `LineMap` anchor is exactly the property
/// under fuzz.
fn arb_tree_base() -> impl Strategy<Value = usize> {
    prop_oneof![IN_BOUNDS_START, FAR_OUT_OF_BOUNDS_START]
}

/// 3 — the same mandatory `Escape` + `Key('h')` prefix `cluster_highlight`
/// uses and for the same two reasons (that cluster's own docs): the edit
/// guarantees the live buffer version is >= 1 before the reply is
/// delivered, so `HighlightVersion::Stale` is genuinely distinct from
/// `Live`; `Escape` unconditionally unparks focus from `Pane::Title` so the
/// edit reaches the Editor regardless of what ran immediately before. The
/// reply itself is synthesized through the TREE channel instead
/// (`Action::HighlightTree`'s own docs): any fixture, any base — `base` is
/// deliberately unvalidated in the same hostile spirit as
/// `arb_highlight_span`'s far-out-of-bounds arm.
fn cluster_highlight_tree() -> impl Strategy<Value = Vec<Action>> {
    (arb_highlight_version(), any::<u8>(), arb_tree_base()).prop_map(|(version, fixture, base)| {
        vec![
            Action::Key(ESCAPE_KEY),
            Action::Key(KeyInput {
                code: KeyCode::Char('h'),
                mods: Mods::NONE,
            }),
            Action::HighlightTree {
                version,
                fixture,
                base,
            },
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
        Just(vec![Action::Key(CTRL_E_KEY)]),
        Just(vec![Action::Key(TRASH_KEY)]),
        Just(vec![Action::Key(FILESEARCH_KEY_CTRL)]),
        Just(vec![Action::Key(FILESEARCH_KEY_SUP)]),
        Just(vec![Action::OpenFileSearch]),
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

/// `DivergeDisk`, `DeliverDbAll` (the probe ack), `^M`, `DeliverDbAll` (the
/// `MergePrep` ack), then exactly ONE of `MERGE_RESOLVE_KEYS`, then a
/// second `^M`: this sequence is what
/// actually carries a session from `Inactive` all the way to `MergeState::
/// Active` and back out again — `DivergeDisk` reclassifies the seeded
/// document toward `DiskAhead`/`Diverged`, the first `DeliverDbAll` lands
/// that reprobe's ack so `MERGE_KEY`'s fast pre-check (`merge::begin`)
/// actually sees it, and the second lands the `MergePrep` ack that
/// installs the working form (`merge::landing`). `DeliverDbAll`, not a
/// single `DeliverDb`: this cluster runs composed with every OTHER cluster
/// in a whole session, any of which can leave its own ops (an
/// `AppendEdit` nobody drained, ...) sitting ahead of this one's `Probe`/
/// `MergePrep` op in the oldest-first queue — a single `DeliverDb` at
/// either checkpoint isn't guaranteed to be THIS sequence's own ack.
///
/// Exactly ONE resolve key, not 1-3: this generator's own seed content is
/// small enough that a diverged session typically produces a SINGLE
/// conflict block, and `merge::resolve::nav`'s own docs ("both directions
/// land back on it" with one block left) mean a SECOND `[`/`]` press in
/// that shape is a genuine, informationless repeat — same cursor, same
/// scroll, same status, correctly caught by `MERGE-KEY-FEEDBACK` as "no
/// observable trace" even though nothing is actually wrong. One resolve
/// key still exercises the whole alphabet (`[`/`]`/`o`/`t`/`b`) across
/// proptest's own sampling, without staking this cluster's `Active`-
/// reachability guarantee on a specific hunk shape.
///
/// The trailing `^M`, not `Escape`, is deliberate: `merge::toggle` (bound
/// in `GLOBAL_BINDINGS`, stage 2 — reachable regardless of which pane is
/// focused) exits an `Active` attempt and is a harmless, focus-inert
/// no-op-with-status otherwise, whereas a bare `Escape` is meaningful only
/// while `Pane::Editor` is focused AND the resolver's own intercept is
/// what's consuming it — the fast, zero-conflict path (`DiskAhead` + a
/// clean buffer) can resolve to `Inactive` on its own before this
/// cluster's `^M` ever lands, and an `Escape` delivered at that point
/// falls through to the ordinary editor's own cascade (collapse
/// selection, THEN hand focus to the Explorer) instead of doing nothing —
/// stranding focus off `Pane::Editor` for every LATER cluster, including
/// this cluster's own next occurrence, whose resolve key and exit chord
/// would then route to the Explorer's key table instead of ever reaching
/// `merge::keys::intercept` again. This cluster is its own self-contained
/// scenario, the same way `cluster_quit_guard` always answers its own
/// Guard before ending — a merge attempt (or a stray focus change) left
/// dangling past this cluster's own boundary is exactly what the `^M`
/// exit avoids.
fn cluster_merge() -> impl Strategy<Value = Vec<Action>> {
    select(MERGE_RESOLVE_KEYS).prop_map(|key| {
        vec![
            Action::DivergeDisk,
            Action::DeliverDbAll,
            Action::Key(MERGE_KEY),
            Action::DeliverDbAll,
            Action::Key(key),
            Action::Key(MERGE_KEY),
        ]
    })
}

/// The user-approved weighted table, now over 17 clusters (plan WP7.S6
/// added `cluster_highlight`; WP14.S1 added `cluster_confirm_stale`;
/// WP14.S3 added `cluster_multicursor`; plan WP2 added `cluster_quit_
/// guard`, the dedicated, self-contained scenario for the `DirtyQuit`
/// Guard's `[S]ave`/`[D]iscard`/`Esc` answers; the merge plan's own WP7.S1
/// added `cluster_merge`, since bumped from a `MERGE_KEY`-only weight of 1
/// once a store-backed session made `MergeState::Active` actually
/// reachable; issue #37's own generator arm added `cluster_highlight_tree`,
/// the TREE-channel twin of `cluster_highlight`). All arms are `.boxed()` —
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
        3 => cluster_highlight_tree().boxed(),
        2 => cluster_async_deliver().boxed(),
        1 => cluster_chrome().boxed(),
        1 => cluster_confirm_stale().boxed(),
        1 => cluster_quit_guard().boxed(),
        3 => cluster_merge().boxed(),
    ]
}

#[cfg(test)]
#[path = "cluster_tests.rs"]
mod tests;
