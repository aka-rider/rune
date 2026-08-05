//! Tests for the `cluster_*` strategies — split out of `cluster.rs` to keep
//! the parent under the file-size ceiling, the same shape `decode_cmd_tests.rs`
//! already uses elsewhere in the workspace.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;

use crate::driver;

use super::*;

/// `cluster_highlight`'s own doc comment claims its `Key('h')` edit is
/// mandatory, but `Action::Key` only reaches the buffer while
/// `focus == Pane::Editor`. Its leading actions must therefore restore
/// Editor focus from whatever pane a preceding cluster left it parked on —
/// a state-free reset, not a claim that holds only from one particular
/// starting pane.
///
/// This matrix samples the real `cluster_highlight` strategy once per case
/// and prepends a different parking prefix ahead of it: every pane the
/// generator can park focus on (Title, Explorer, Tabs, Explorer live-search,
/// Messages, a mid-quit-confirm dialog) plus the fresh-`Editor` base case
/// and a tiny-terminal probe. Every case must settle with no invariant
/// violation and the guaranteed 'h' edit in the buffer.
///
/// Run this with `cluster_highlight`'s leading reset reverted to a bare
/// `Action::Key(ESCAPE_KEY)` to see it regress: the fresh-Editor case fails
/// because a lone Escape with nothing focused elsewhere is a no-op walked
/// into empty space, and the Explorer live-search case fails because
/// Escape there clears the search text instead of returning focus, in both
/// cases leaving `final_content` empty instead of `"h"`.
struct ParkCase {
    label: &'static str,
    prefix: Vec<Action>,
}

fn park_cases() -> Vec<ParkCase> {
    vec![
        ParkCase {
            label: "no prefix (fresh Editor)",
            prefix: vec![],
        },
        ParkCase {
            label: "^R (Title)",
            prefix: vec![Action::Key(CTRL_R_KEY)],
        },
        ParkCase {
            label: "^B (Explorer)",
            prefix: vec![Action::Key(CTRL_B_KEY)],
        },
        ParkCase {
            label: "^T (Tabs)",
            prefix: vec![Action::Key(CTRL_T_KEY)],
        },
        ParkCase {
            label: "^B, 'r' (Explorer live-search)",
            prefix: vec![
                Action::Key(CTRL_B_KEY),
                Action::Key(KeyInput {
                    code: KeyCode::Char('r'),
                    mods: Mods::NONE,
                }),
            ],
        },
        ParkCase {
            label: "^E (Messages)",
            prefix: vec![Action::Key(CTRL_E_KEY)],
        },
        ParkCase {
            label: "^C (mid-quit-confirm)",
            prefix: vec![Action::Key(CTRL_C_KEY)],
        },
        ParkCase {
            label: "tiny terminal (Resize to arb_resize's minimum)",
            prefix: vec![Action::Resize(1, 2)],
        },
    ]
}

#[test]
fn cluster_highlight_edit_survives_focus_parked_off_editor() {
    let mut runner = TestRunner::default();

    for case in park_cases() {
        let tree = cluster_highlight()
            .new_tree(&mut runner)
            .expect("cluster_highlight strategy generation failed");

        let mut actions = case.prefix;
        actions.extend(tree.current());

        let result = driver::run(driver::DOC_PATH, "", &actions);

        assert_eq!(
            result.violation.as_ref().map(|v| v.id),
            None,
            "[{}] session should settle with no invariant violation",
            case.label
        );
        assert_eq!(
            result.final_content, "h",
            "[{}] cluster_highlight's guaranteed edit must land the 'h' keystroke in the \
             buffer regardless of where focus was parked beforehand; got {:?} instead",
            case.label, result.final_content
        );
    }
}
