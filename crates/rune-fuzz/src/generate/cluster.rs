//! The `cluster_*` strategy functions and the weighted table over them,
//! split out of `generate` (§1.6 budget) — every one of these draws its
//! fixed data from `palette.rs`.

use proptest::prelude::*;
use proptest::sample::select;

use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

use crate::action::Action;

use super::palette::{
    COPY_KEY, CTRL_C_KEY, CTRL_R_KEY, CUT_KEY, DELETE_KEYS, ENTER_KEY, MARKDOWN_FRAGMENTS,
    NAV_KEYS, PASTE_KEY, PASTE_PALETTE, REDO_KEY, SAVE_KEY, SELECT_ALL_KEY, SELECT_MOTION_KEYS,
    TYPE_PALETTE, UNDO_KEY,
};

fn arb_resize() -> impl Strategy<Value = (u16, u16)> {
    (1u16..=200, 2u16..=60)
}

/// Any of the 15 `KeyCode` variants; `Char` draws an arbitrary `char`.
/// 15 arms exceeds `prop_oneof!`'s 10-arm threshold (G16), so every arm is
/// `.boxed()`.
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
    ("[a-zA-Z0-9_.]{0,12}", any::<bool>()).prop_map(|(name, is_dir)| DirEntry { name, is_dir })
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

/// 1 — one of `Resize`, `FailNextSave`, `Key(ctrl+c)`, `ConfirmTimeout`, or
/// `DirLoaded` with 0-6 arbitrary entries (plan WP4.S6).
fn cluster_chrome() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        arb_resize().prop_map(|(w, h)| vec![Action::Resize(w, h)]),
        Just(vec![Action::FailNextSave]),
        Just(vec![Action::Key(CTRL_C_KEY)]),
        Just(vec![Action::Key(CTRL_R_KEY)]),
        Just(vec![Action::ConfirmTimeout]),
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

/// The user-approved weighted table over 11 clusters. All 11 arms are
/// `.boxed()` — `prop_oneof!` with >10 arms expands to
/// `Union::new_weighted(vec![…boxed…])` (G16).
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
        3 => cluster_monkey_burst().boxed(),
        2 => cluster_async_deliver().boxed(),
        1 => cluster_chrome().boxed(),
    ]
}
