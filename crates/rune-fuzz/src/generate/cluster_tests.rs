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

/// FINDING A regression: `cluster_highlight`'s own doc comment claims
/// its `Key('h')` edit is mandatory, but `Action::Key` only reaches the
/// buffer while `focus == Pane::Editor` — and a preceding cluster can
/// leave focus elsewhere with no restore (`cluster_chrome`'s
/// `Key(CTRL_R_KEY)` arm parks it on `Pane::Title`). This test starts a
/// session with exactly that arm, THEN runs whatever `cluster_highlight`
/// itself generates — sampled from the real strategy, not a hand-copied
/// stand-in — and asserts the edit still lands. Run this with the
/// `Action::Key(ESCAPE_KEY)`/`Action::Key(CTRL_E_KEY)` prefix reverted
/// to confirm it fails first (recorded in the fix's commit/handoff, not
/// re-derivable from the test alone).
#[test]
fn cluster_highlight_edit_survives_focus_parked_off_editor() {
    let mut runner = TestRunner::default();
    let tree = cluster_highlight()
        .new_tree(&mut runner)
        .expect("cluster_highlight strategy generation failed");

    // Mirrors `cluster_chrome`'s no-restore `Key(CTRL_R_KEY)` arm: parks
    // focus on `Pane::Title` before `cluster_highlight`'s own actions
    // run, with no subsequent focus restore of any kind.
    let mut actions = vec![Action::Key(CTRL_R_KEY)];
    actions.extend(tree.current());

    let result = driver::run(driver::DOC_PATH, "", &actions);

    assert_eq!(
        result.violation.as_ref().map(|v| v.id),
        None,
        "session should settle with no invariant violation"
    );
    assert_eq!(
        result.final_content, "h",
        "cluster_highlight's guaranteed edit must land the 'h' keystroke in the buffer even \
         when a preceding action parks focus off the editor; got {:?} instead",
        result.final_content
    );
}
