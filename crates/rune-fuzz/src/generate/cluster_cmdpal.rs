use proptest::prelude::*;
use proptest::sample::select;

use rune_tui::keymap::KeyInput;

use crate::action::{Action, PaletteGenClaim};

use super::palette::{
    CMDPAL_BACKSPACE_KEY, CMDPAL_KEY_CTRL, CMDPAL_KEY_SUP, CMDPAL_NAV_KEYS, CMDPAL_PARAM_QUERIES,
    CMDPAL_TAB_KEY, CTRL_B_KEY, CTRL_R_KEY, CTRL_T_KEY, ENTER_KEY, ESCAPE_KEY, FILESEARCH_KEY_CTRL,
    FILESEARCH_KEY_SUP, TYPE_PALETTE,
};

fn cmdpal_open() -> impl Strategy<Value = KeyInput> {
    prop_oneof![Just(CMDPAL_KEY_CTRL), Just(CMDPAL_KEY_SUP)]
}

pub(super) fn cluster_cmdpal_type() -> impl Strategy<Value = Vec<Action>> {
    (
        cmdpal_open(),
        proptest::collection::vec(select(TYPE_PALETTE), 1..=3),
    )
        .prop_map(|(open, frags)| vec![Action::Key(open), Action::Type(frags.join(" "))])
}

pub(super) fn cluster_cmdpal_navigate() -> impl Strategy<Value = Vec<Action>> {
    (
        cmdpal_open(),
        proptest::collection::vec(select(CMDPAL_NAV_KEYS), 1..=5),
    )
        .prop_map(|(open, keys)| {
            let mut actions = vec![Action::Key(open)];
            actions.extend(keys.into_iter().map(Action::Key));
            actions
        })
}

pub(super) fn cluster_cmdpal_param_flow() -> impl Strategy<Value = Vec<Action>> {
    (
        cmdpal_open(),
        select(CMDPAL_PARAM_QUERIES),
        select(TYPE_PALETTE),
    )
        .prop_map(|(open, query, arg_frag)| {
            vec![
                Action::Key(open),
                Action::Type(query.to_string()),
                Action::Key(CMDPAL_TAB_KEY),
                Action::Type(arg_frag.to_string()),
                Action::Key(ENTER_KEY),
            ]
        })
}

pub(super) fn cluster_cmdpal_save() -> impl Strategy<Value = Vec<Action>> {
    cmdpal_open().prop_map(|open| {
        vec![
            Action::Key(open),
            Action::Type("save".to_string()),
            Action::Key(ENTER_KEY),
        ]
    })
}

pub(super) fn cluster_cmdpal_backspace() -> impl Strategy<Value = Vec<Action>> {
    (cmdpal_open(), select(CMDPAL_PARAM_QUERIES), 1u8..=6).prop_map(|(open, query, n)| {
        let mut actions = vec![
            Action::Key(open),
            Action::Type(query.to_string()),
            Action::Key(CMDPAL_TAB_KEY),
        ];
        actions.extend(std::iter::repeat_n(
            Action::Key(CMDPAL_BACKSPACE_KEY),
            n as usize,
        ));
        actions
    })
}

pub(super) fn cluster_cmdpal_escape() -> impl Strategy<Value = Vec<Action>> {
    (cmdpal_open(), select(TYPE_PALETTE)).prop_map(|(open, frag)| {
        vec![
            Action::Key(open),
            Action::Type(frag.to_string()),
            Action::Key(ESCAPE_KEY),
        ]
    })
}

pub(super) fn cluster_cmdpal_global_interleave() -> impl Strategy<Value = Vec<Action>> {
    (
        cmdpal_open(),
        select(TYPE_PALETTE),
        prop_oneof![
            Just(CTRL_T_KEY),
            Just(CTRL_B_KEY),
            Just(CTRL_R_KEY),
            Just(FILESEARCH_KEY_CTRL),
            Just(FILESEARCH_KEY_SUP),
        ],
    )
        .prop_map(|(open, frag, chord)| {
            vec![
                Action::Key(open),
                Action::Type(frag.to_string()),
                Action::Key(chord),
            ]
        })
}

pub(super) fn cluster_cmdpal_recents() -> impl Strategy<Value = Vec<Action>> {
    (
        proptest::option::of(cmdpal_open()),
        prop_oneof![
            Just(PaletteGenClaim::Live),
            any::<u32>().prop_map(PaletteGenClaim::Stale),
        ],
        proptest::bool::weighted(0.85),
        proptest::collection::vec(select(TYPE_PALETTE), 0..=5),
    )
        .prop_map(|(open, generation, ok, frags)| {
            let mut actions = Vec::new();
            if let Some(key) = open {
                actions.push(Action::Key(key));
            }
            actions.push(Action::PaletteRecentsLoaded {
                generation,
                ok,
                names: frags.into_iter().map(str::to_string).collect(),
            });
            actions
        })
}

pub(super) fn cluster_cmdpal() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        4 => cluster_cmdpal_type().boxed(),
        3 => cluster_cmdpal_navigate().boxed(),
        2 => cluster_cmdpal_param_flow().boxed(),
        2 => cluster_cmdpal_save().boxed(),
        2 => cluster_cmdpal_backspace().boxed(),
        2 => cluster_cmdpal_escape().boxed(),
        2 => cluster_cmdpal_global_interleave().boxed(),
        1 => cluster_cmdpal_recents().boxed(),
    ]
}
