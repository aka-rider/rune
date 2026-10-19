use proptest::prelude::*;
use proptest::sample::select;

use rune_tui::keymap::KeyInput;
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};

use crate::action::Action;

use super::palette::{
    EDITOR_CLICK_CELL, ESCAPE_KEY, FIND_CHARS, FIND_KEY_CTRL, FIND_PANEL_KEYS,
    PROJECT_FIND_KEY_CTRL, PROJECT_REPLACE_KEY_CTRL, REPLACE_ALL_KEY, REPLACE_KEY_CTRL,
    RESULTS_KEYS,
};

fn panel_open() -> impl Strategy<Value = KeyInput> {
    prop_oneof![
        Just(FIND_KEY_CTRL),
        Just(REPLACE_KEY_CTRL),
        Just(PROJECT_FIND_KEY_CTRL),
    ]
}

fn typed() -> impl Strategy<Value = String> {
    proptest::collection::vec(select(FIND_CHARS), 1..=4)
        .prop_map(|chars| chars.into_iter().collect())
}

fn panel_keys(max: usize) -> impl Strategy<Value = Vec<Action>> {
    proptest::collection::vec(select(FIND_PANEL_KEYS), 0..=max)
        .prop_map(|keys| keys.into_iter().map(Action::Key).collect())
}

pub(super) fn click_into_editor() -> [Action; 2] {
    [
        MouseKind::Down(MouseButton::Left),
        MouseKind::Up(MouseButton::Left),
    ]
    .map(|kind| {
        Action::Mouse(MouseInput {
            kind,
            column: EDITOR_CLICK_CELL.0,
            row: EDITOR_CLICK_CELL.1,
            shift: false,
            alt: false,
            ctrl: false,
        })
    })
}

fn results_visit() -> impl Strategy<Value = Vec<Action>> {
    proptest::bool::ANY.prop_map(|visit| {
        if visit {
            RESULTS_KEYS.iter().copied().map(Action::Key).collect()
        } else {
            Vec::new()
        }
    })
}

pub(super) fn cluster_find() -> impl Strategy<Value = Vec<Action>> {
    (
        panel_open(),
        typed(),
        panel_keys(6),
        results_visit(),
        proptest::option::of((typed(), panel_keys(3))),
        proptest::bool::weighted(0.4),
    )
        .prop_map(|(open, query, keys, results, replace, click)| {
            let mut actions = vec![Action::Key(open), Action::Type(query)];
            actions.extend(keys);
            if open == PROJECT_FIND_KEY_CTRL {
                actions.extend(results);
            }
            if let Some((replacement, keys)) = replace {
                if open == PROJECT_FIND_KEY_CTRL {
                    actions.push(Action::Key(PROJECT_REPLACE_KEY_CTRL));
                    actions.push(Action::Type(replacement));
                    actions.push(Action::Key(REPLACE_ALL_KEY));
                } else {
                    actions.push(Action::Key(REPLACE_KEY_CTRL));
                    actions.push(Action::Type(replacement));
                }
                actions.extend(keys);
            }
            if click {
                actions.extend(click_into_editor());
            }
            actions.push(Action::Key(ESCAPE_KEY));
            actions
        })
}
