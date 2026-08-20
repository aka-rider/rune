//! The `cluster_*` strategy functions and the weighted table over them,
//! split out of `generate` (500-line budget) — every one of these
//! draws its fixed data from `palette.rs`, composing the raw value
//! generators in `arb.rs` into `Vec<Action>` sequences.

use proptest::prelude::*;

use crate::action::Action;

mod cluster_input;
mod cluster_special;
mod cluster_cmdpal;

#[cfg(test)]
pub(super) use super::arb::{RESIZE_MIN_HEIGHT, RESIZE_MIN_WIDTH};

pub(super) use cluster_input::{
    cluster_type_prose, cluster_navigate, cluster_selection, cluster_delete, cluster_undo_redo,
    cluster_caret_history, cluster_advance_clock, cluster_markdown_write, cluster_save,
    cluster_clipboard, cluster_monkey_burst, cluster_highlight, cluster_highlight_tree,
    cluster_async_deliver,
};

pub(super) use cluster_special::{
    cluster_chrome, cluster_multicursor, cluster_mouse, cluster_confirm_stale, cluster_quit_guard,
    cluster_merge,
};

pub(super) use cluster_cmdpal::{
    cluster_cmdpal,
};

pub(super) fn arb_cluster() -> impl Strategy<Value = Vec<Action>> {
    prop_oneof![
        35 => cluster_type_prose().boxed(),
        22 => cluster_navigate().boxed(),
        10 => cluster_selection().boxed(),
        8 => cluster_delete().boxed(),
        7 => cluster_undo_redo().boxed(),
        5 => cluster_caret_history().boxed(),
        2 => cluster_advance_clock().boxed(),
        6 => cluster_markdown_write().boxed(),
        5 => cluster_save().boxed(),
        4 => cluster_clipboard().boxed(),
        4 => cluster_multicursor().boxed(),
        3 => cluster_monkey_burst().boxed(),
        3 => cluster_highlight().boxed(),
        3 => cluster_highlight_tree().boxed(),
        2 => cluster_async_deliver().boxed(),
        2 => cluster_mouse().boxed(),
        1 => cluster_chrome().boxed(),
        1 => cluster_confirm_stale().boxed(),
        1 => cluster_quit_guard().boxed(),
        3 => cluster_merge().boxed(),
        6 => cluster_cmdpal().boxed(),
    ]
}

#[cfg(test)]
#[path = "cluster_tests.rs"]
mod tests;
