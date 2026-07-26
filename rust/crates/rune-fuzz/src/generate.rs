//! The session generator. WP3 keeps this deliberately simple: a UNIFORM
//! `prop_oneof!` over the eight `Action` variants. WP6 replaces the weights
//! with the user-approved "normal human session" table (a `prop_oneof!`
//! over clusters instead of bare actions) without changing `arb_session`'s
//! signature — every caller (the fuzz target, the tripwire round-trip test,
//! the replay test) keeps working unmodified.

use proptest::prelude::*;
use proptest::sample::select;

use rune_tui::keymap::{KeyCode, KeyInput, Mods};

use crate::action::Action;

/// Seed documents sessions start from. `select` panics on an empty slice
/// (G16) — never let this list go empty.
static CONTENT_SEEDS: &[&str] = &[
    "",
    "hello world\n",
    "# Title\n\nSome prose here.\n",
    "line one\nline two\nline three\n",
];

/// `Type`/`Paste`/`ClipboardReply` payloads. Deliberately control-character
/// free (including no `\t`/`\r`) — WP3's generator restricts itself to a
/// palette that trivially satisfies `Action::Type`'s "no non-`\n` control
/// char" contract (plan Gotcha G3) for every action that can carry it. WP6
/// replaces this with Go's full byte-hostile `PASTE_PALETTE`/`TYPE_PALETTE`
/// split.
static TEXT_FRAGMENTS: &[&str] = &[
    "hello",
    "world",
    "the quick brown fox",
    "annotate this",
    "1234567890",
    "café",
    "日本語のテスト",
    "🙂 emoji mix 👍",
    "under_score-dash",
];

/// A representative set of keystrokes: navigation, editing, clipboard,
/// undo/redo, save, and both quit chords. `select` needs a `&'static [T]`
/// (G16).
static KEY_TABLE: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Left,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Right,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Up,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Down,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Home,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::End,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::PageUp,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::PageDown,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Backspace,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Delete,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Tab,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::BackTab,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Left,
        mods: Mods {
            shift: true,
            ..Mods::NONE
        },
    },
    KeyInput {
        code: KeyCode::Char('a'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    },
    KeyInput {
        code: KeyCode::Char('c'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    },
    KeyInput {
        code: KeyCode::Char('x'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    },
    KeyInput {
        code: KeyCode::Char('v'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    },
    KeyInput {
        code: KeyCode::Char('z'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    },
    KeyInput {
        code: KeyCode::Char('z'),
        mods: Mods {
            sup: true,
            shift: true,
            ..Mods::NONE
        },
    },
    KeyInput {
        code: KeyCode::Char('s'),
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    },
    KeyInput {
        code: KeyCode::Char('c'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    },
    KeyInput {
        code: KeyCode::Char('d'),
        mods: Mods {
            ctrl: true,
            alt: true,
            ..Mods::NONE
        },
    },
];

fn arb_text() -> impl Strategy<Value = String> {
    select(TEXT_FRAGMENTS).prop_map(str::to_string)
}

fn arb_key() -> impl Strategy<Value = KeyInput> {
    select(KEY_TABLE)
}

fn arb_resize() -> impl Strategy<Value = (u16, u16)> {
    (1u16..=200, 2u16..=60)
}

/// One `Action`. Eight arms, uniformly weighted — `prop_oneof!` only needs
/// every arm `.boxed()` once it exceeds ten arms (G16), so this doesn't.
fn arb_action() -> impl Strategy<Value = Action> {
    prop_oneof![
        arb_key().prop_map(Action::Key),
        arb_text().prop_map(Action::Type),
        arb_text().prop_map(Action::Paste),
        arb_resize().prop_map(|(w, h)| Action::Resize(w, h)),
        arb_text().prop_map(Action::ClipboardReply),
        Just(Action::ConfirmTimeout),
        Just(Action::Deliver),
        Just(Action::FailNextSave),
    ]
}

/// One whole fuzz case: the seed content plus a 1..=120 action session
/// (plan Assumption A3, mirroring Go's `maxHumanEvents = 160`).
pub fn arb_session() -> impl Strategy<Value = (String, Vec<Action>)> {
    (
        select(CONTENT_SEEDS).prop_map(str::to_string),
        proptest::collection::vec(arb_action(), 1..=120),
    )
}
