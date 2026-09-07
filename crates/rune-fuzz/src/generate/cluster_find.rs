use proptest::prelude::*;
use proptest::sample::select;

use rune_tui::keymap::KeyInput;

use crate::action::Action;

use super::palette::{
    ESCAPE_KEY, FIND_CHARS, FIND_KEY_CTRL, FIND_PANEL_KEYS, PROJECT_FIND_KEY_CTRL,
    REPLACE_KEY_CTRL, RESULTS_KEYS,
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
    )
        .prop_map(|(open, query, keys, results, replace)| {
            let mut actions = vec![Action::Key(open), Action::Type(query)];
            actions.extend(keys);
            if open == PROJECT_FIND_KEY_CTRL {
                actions.extend(results);
            }
            if let Some((replacement, keys)) = replace {
                actions.push(Action::Key(REPLACE_KEY_CTRL));
                actions.push(Action::Type(replacement));
                actions.extend(keys);
            }
            actions.push(Action::Key(ESCAPE_KEY));
            actions
        })
}
