use proptest::prelude::*;
use proptest::sample::select;

use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};

use crate::action::Action;

use super::arb::{arb_dir_cause, arb_dir_entry, arb_dir_loaded_generation, arb_mouse_button, arb_mouse_cell, arb_mouse_input, arb_resize};
use super::palette::{
    ADD_CURSOR_ABOVE_KEY, ADD_CURSOR_BELOW_KEY, COPY_KEY, CTRL_B_KEY, CTRL_C_KEY, CTRL_E_KEY,
    CTRL_P_KEY, CTRL_R_KEY, CTRL_T_KEY, ESCAPE_KEY, EXPLORER_SEARCH_KEYS, FILESEARCH_KEY_CTRL,
    FILESEARCH_KEY_SUP, MERGE_KEY, MERGE_RESOLVE_KEYS,
    TITLE_MOTION_KEYS, TRASH_KEY, TYPE_PALETTE,
};

pub(super) fn cluster_chrome() -> impl Strategy<Value = Vec<Action>> {
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

pub(super) fn cluster_multicursor() -> impl Strategy<Value = Vec<Action>> {
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

pub(super) fn cluster_mouse() -> impl Strategy<Value = Vec<Action>> {
    let press_release = |button: MouseButton, (column, row): (u16, u16)| {
        [MouseKind::Down(button), MouseKind::Up(button)].map(|kind| {
            Action::Mouse(MouseInput {
                kind,
                column,
                row,
                shift: false,
                alt: false,
                ctrl: false,
            })
        })
    };
    prop_oneof![
        3 => (arb_mouse_button(), arb_mouse_cell(), 1usize..=3).prop_map(
            move |(button, cell, clicks)| {
                (0..clicks).flat_map(|_| press_release(button, cell)).collect()
            }
        ),
        2 => (arb_mouse_cell(), arb_mouse_cell()).prop_map(|(from, to)| {
            vec![
                Action::Mouse(MouseInput {
                    kind: MouseKind::Down(MouseButton::Left),
                    column: from.0,
                    row: from.1,
                    shift: false,
                    alt: false,
                    ctrl: false,
                }),
                Action::Mouse(MouseInput {
                    kind: MouseKind::Drag(MouseButton::Left),
                    column: to.0,
                    row: to.1,
                    shift: false,
                    alt: false,
                    ctrl: false,
                }),
                Action::Mouse(MouseInput {
                    kind: MouseKind::Up(MouseButton::Left),
                    column: to.0,
                    row: to.1,
                    shift: false,
                    alt: false,
                    ctrl: false,
                }),
            ]
        }),
        2 => (
            prop_oneof![Just(MouseKind::ScrollUp), Just(MouseKind::ScrollDown)],
            arb_mouse_cell(),
            1usize..=3
        )
            .prop_map(|(kind, (column, row), ticks)| {
                vec![
                    Action::Mouse(MouseInput {
                        kind,
                        column,
                        row,
                        shift: false,
                        alt: false,
                        ctrl: false,
                    });
                    ticks
                ]
            }),
        1 => arb_mouse_input().prop_map(|m| vec![Action::Mouse(m)]),
    ]
}

pub(super) fn cluster_confirm_stale() -> impl Strategy<Value = Vec<Action>> {
    Just(vec![
        Action::Key(CTRL_C_KEY),
        Action::Key(ESCAPE_KEY),
        Action::StaleConfirmTimeout(u32::MAX),
    ])
}

pub(super) fn cluster_quit_guard() -> impl Strategy<Value = Vec<Action>> {
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

pub(super) fn cluster_merge() -> impl Strategy<Value = Vec<Action>> {
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
