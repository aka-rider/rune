use proptest::prelude::*;
use proptest::sample::select;

use rune_tui::keymap::{KeyCode, KeyInput, Mods};

use crate::action::Action;

use super::arb::{
    FAR_OUT_OF_BOUNDS_START, IN_BOUNDS_START, arb_any_keycode, arb_clock_advance_millis,
    arb_highlight_span, arb_highlight_version, arb_mods,
};
use super::palette::{
    COPY_KEY, CTRL_T_KEY, CUT_KEY, DELETE_KEYS, ENTER_KEY, ESCAPE_KEY, MARKDOWN_FRAGMENTS,
    NAV_BACK_KEY, NAV_FORWARD_KEY, NAV_KEYS, PASTE_KEY, PASTE_PALETTE, REDO_KEY, REDO_KEY_ALT,
    SAVE_KEY, SELECT_ALL_KEY, SELECT_MOTION_KEYS, TYPE_PALETTE, UNDO_KEY,
};

pub(super) fn cluster_type_prose() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        3 => proptest::collection::vec(select(TYPE_PALETTE), 1..=4)
            .prop_map(|frags| vec![Action::Type(frags.join(" "))]),
        1 => select(PASTE_PALETTE).prop_map(|s| vec![Action::Paste(s.to_string())]),
    ]
}

pub(super) fn cluster_navigate() -> impl Strategy<Value = Vec<Action>> {
    proptest::collection::vec(select(NAV_KEYS), 1..=6)
        .prop_map(|keys| keys.into_iter().map(Action::Key).collect())
}

pub(super) fn cluster_selection() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        proptest::collection::vec(select(SELECT_MOTION_KEYS), 1..=5)
            .prop_map(|keys| keys.into_iter().map(Action::Key).collect()),
        Just(vec![Action::Key(SELECT_ALL_KEY)]),
    ]
}

pub(super) fn cluster_delete() -> impl Strategy<Value = Vec<Action>> {
    proptest::collection::vec(select(DELETE_KEYS), 1..=6)
        .prop_map(|keys| keys.into_iter().map(Action::Key).collect())
}

pub(super) fn cluster_undo_redo() -> impl Strategy<Value = Vec<Action>> {
    (1usize..=4, proptest::option::of(1usize..=3)).prop_flat_map(|(undo_n, redo_n)| {
        let actions = vec![Action::Key(UNDO_KEY); undo_n];
        match redo_n {
            Some(n) => (0..n)
                .map(|_| prop_oneof![Just(REDO_KEY), Just(REDO_KEY_ALT)])
                .collect::<Vec<_>>()
                .prop_map(move |redo_keys| {
                    let mut result = actions.clone();
                    result.extend(redo_keys.into_iter().map(Action::Key));
                    result
                })
                .boxed(),
            None => Just(actions).boxed(),
        }
    })
}

pub(super) fn cluster_caret_history() -> impl Strategy<Value = Vec<Action>> {
    proptest::collection::vec(
        prop_oneof![Just(NAV_BACK_KEY), Just(NAV_FORWARD_KEY)],
        1..=4,
    )
    .prop_map(|keys| keys.into_iter().map(Action::Key).collect())
}

pub(super) fn cluster_advance_clock() -> impl Strategy<Value = Vec<Action>> {
    arb_clock_advance_millis().prop_map(|millis| vec![Action::AdvanceClock(millis)])
}

pub(super) fn cluster_markdown_write() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        7 => select(MARKDOWN_FRAGMENTS).prop_map(|s| vec![Action::Type(s.to_string())]),
        1 => Just(vec![Action::Type("```rust".to_string()), Action::Key(ENTER_KEY)]),
    ]
}

pub(super) fn cluster_save() -> impl Strategy<Value = Vec<Action>> {
    proptest::bool::weighted(0.75).prop_map(|deliver| {
        let mut actions = vec![Action::Key(SAVE_KEY)];
        if deliver {
            actions.push(Action::Deliver);
        }
        actions
    })
}

pub(super) fn cluster_clipboard() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        Just(vec![Action::Key(COPY_KEY)]),
        Just(vec![Action::Key(CUT_KEY)]),
        select(PASTE_PALETTE).prop_map(|s| vec![
            Action::Key(PASTE_KEY),
            Action::ClipboardReply(s.to_string())
        ]),
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

pub(super) fn cluster_monkey_burst() -> impl Strategy<Value = Vec<Action>> {
    proptest::collection::vec(
        (arb_any_keycode(), arb_mods()).prop_map(|(code, mods)| KeyInput { code, mods }),
        3..=12,
    )
    .prop_map(|keys| keys.into_iter().map(Action::Key).collect())
}

pub(super) fn cluster_highlight() -> impl Strategy<Value = Vec<Action>> {
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

fn arb_tree_base() -> impl Strategy<Value = usize> {
    prop_oneof![IN_BOUNDS_START, FAR_OUT_OF_BOUNDS_START]
}

pub(super) fn cluster_highlight_tree() -> impl Strategy<Value = Vec<Action>> {
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

pub(super) fn cluster_async_deliver() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        Just(vec![Action::Deliver]),
        Just(vec![Action::Deliver, Action::DeliverDb]),
        Just(vec![Action::DeliverDb]),
        Just(vec![Action::DeliverDbAll]),
    ]
}
