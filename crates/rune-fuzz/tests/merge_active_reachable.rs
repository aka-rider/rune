//! Acceptance anchor for issue 35: proves the store-backed session can
//! genuinely reach `MergeState::Active` through the REAL driver entry point
//! (`driver::run`), independent of the generator's random weights — the
//! four merge invariants (`invariant/merge.rs`) are non-vacuous only if
//! SOME reachable action sequence lands here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rune_fuzz::action::Action;
use rune_fuzz::driver;

fn assert_clean(result: &driver::RunResult) {
    assert!(
        result.violation.is_none(),
        "{}",
        result
            .violation
            .as_ref()
            .map(|v| format!("{}: {}", v.id, v.message))
            .unwrap_or_default()
    );
}

/// A fixed, non-random action list — `DivergeDisk` reclassifies the seeded
/// document toward `Diverged`, the first `DeliverDb` lands that reprobe's
/// ack, `^M` starts the merge attempt, and the second `DeliverDb` lands the
/// `MergePrep` ack that installs the working form (`merge::landing`).
#[test]
fn a_scripted_diverge_then_merge_sequence_reaches_active() {
    let actions = vec![
        // Ours changes (one journaled edit) — the buffer diverges from the
        // ancestor the initial `Load` recorded.
        Action::Type("!".to_string()),
        Action::DeliverDb, // the typed edit's own AppendEdit ack
        // Theirs changes too — both sides moved since the ancestor, so the
        // reprobe below must classify Diverged, not the DiskAhead clean
        // fast path (`merge_on_a_disk_ahead_clean_document_installs_disk_
        // bytes_with_no_markers`'s own sibling case, which never activates
        // the resolver at all).
        Action::DivergeDisk,
        Action::DeliverDb, // the reprobe's own Probe ack
        Action::Key(rune_tui::keymap::KeyInput {
            code: rune_tui::keymap::KeyCode::Char('m'),
            mods: rune_tui::keymap::Mods {
                ctrl: true,
                ..rune_tui::keymap::Mods::NONE
            },
        }),
        Action::DeliverDb, // the MergePrep ack that installs the working form
        // Exits the resolver in place before the session ends via a SECOND
        // `^M` (`merge::toggle`), never a bare `Escape`: `Escape` is
        // meaningful only through the resolver's own intercept while
        // `Pane::Editor` is focused, and this driver's end-of-session
        // undo/redo sweep would otherwise repeatedly re-press whatever key
        // is left bound, which is its own, separate question this
        // acceptance anchor isn't scoped to explore (`cluster_merge`'s own
        // doc comment carries the full rationale).
        Action::Key(rune_tui::keymap::KeyInput {
            code: rune_tui::keymap::KeyCode::Char('m'),
            mods: rune_tui::keymap::Mods {
                ctrl: true,
                ..rune_tui::keymap::Mods::NONE
            },
        }),
    ];
    let result = driver::run("/fuzz/doc.md", "hello", &actions);
    assert_clean(&result);
    assert!(
        result.merge_activated,
        "expected the session to reach MergeState::Active at some point"
    );
}

/// Regression anchor: a session that ends with merge still `Active` (no
/// closing `^M`) must let the end-of-session undo sweep
/// (`checks::drive_end_of_session_checks`) reach `journal_pos == 0` clean —
/// the fifth `restore_editor_focus` step exits the resolver in place
/// (`Escape` -> `MergeCommand::Exit`) before the sweep's own `⌘Z` presses
/// begin, exactly like a user would. Before that restore existed, every
/// `⌘Z` the sweep pressed was swallowed by the resolver's own intercept
/// instead of ever reaching the journal.
#[test]
fn a_session_ending_with_merge_active_still_drives_journal_pos_to_zero() {
    let actions = vec![
        Action::Type("!".to_string()),
        Action::DeliverDb,
        Action::DivergeDisk,
        Action::DeliverDb,
        Action::Key(rune_tui::keymap::KeyInput {
            code: rune_tui::keymap::KeyCode::Char('m'),
            mods: rune_tui::keymap::Mods {
                ctrl: true,
                ..rune_tui::keymap::Mods::NONE
            },
        }),
        Action::DeliverDb,
        // No closing `^M`/`Escape` here: the session ends with merge still
        // `Active`, unlike the acceptance anchor above.
    ];
    let result = driver::run("/fuzz/doc.md", "hello", &actions);
    assert_clean(&result);
    assert!(
        result.merge_activated,
        "expected the session to reach MergeState::Active at some point"
    );
}

/// Negative control: the same seed with no divergence ever raised must
/// never spuriously report reaching `Active` — proves `merge_activated`
/// tracks the real thing, not a permanently-latched default.
#[test]
fn a_session_with_no_divergence_never_reaches_active() {
    let actions = vec![
        Action::Type("hello".to_string()),
        Action::Key(rune_tui::keymap::KeyInput {
            code: rune_tui::keymap::KeyCode::Char('m'),
            mods: rune_tui::keymap::Mods {
                ctrl: true,
                ..rune_tui::keymap::Mods::NONE
            },
        }),
    ];
    let result = driver::run("/fuzz/doc.md", "hello", &actions);
    assert_clean(&result);
    assert!(
        !result.merge_activated,
        "no divergence was ever raised, so merge must never have gone Active"
    );
}
