//! WP6.S5 detection tests: `REDO-CLEAR`, `UNDO-TOTAL`, `REDO-TOTAL`.

use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_fuzz::invariant::{redo_clear, redo_total, undo_total};
use rune_tui::keymap::{KeyCode, Mods};

use crate::support::{base_snapshot, key};

// ---------------------------------------------------------------------
// REDO-CLEAR
// ---------------------------------------------------------------------

#[test]
fn redo_clear_detects_a_stale_redo_tail_after_a_new_edit() {
    let mut prev = base_snapshot("abc");
    prev.version = 1;
    prev.journal_len = 2;
    prev.journal_pos = 2; // no redo tail before this edit
    let mut next = base_snapshot("abcd");
    next.version = 2; // a new edit landed
    next.journal_len = 3; // Journal::push grew the log
    next.journal_pos = 2; // but pos was NOT advanced to journal_len — bug
    let v = redo_clear(&prev, &next)
        .expect("a new edit that doesn't clear the redo tail must trip REDO-CLEAR");
    assert_eq!(v.id, "REDO-CLEAR");
}

#[test]
fn redo_clear_accepts_a_new_edit_that_clears_the_tail() {
    let mut prev = base_snapshot("abc");
    prev.version = 1;
    prev.journal_len = 2;
    prev.journal_pos = 2;
    let mut next = base_snapshot("abcd");
    next.version = 2;
    next.journal_len = 3;
    next.journal_pos = 3; // journal_pos == journal_len: correctly cleared
    assert_eq!(redo_clear(&prev, &next), None);
}

#[test]
fn redo_clear_accepts_a_pure_undo_move_with_unchanged_journal_len() {
    // version bumps (G5) but journal_len is unchanged (a move_pos, not a
    // push) — REDO-CLEAR must not fire.
    let mut prev = base_snapshot("abcd");
    prev.version = 2;
    prev.journal_len = 2;
    prev.journal_pos = 2;
    let mut next = base_snapshot("abc");
    next.version = 3;
    next.journal_len = 2;
    next.journal_pos = 1;
    assert_eq!(redo_clear(&prev, &next), None);
}

// ---------------------------------------------------------------------
// UNDO-TOTAL / REDO-TOTAL
// ---------------------------------------------------------------------

/// A short real session, driven end to end through `driver::run` (which
/// now always performs the `UNDO-TOTAL`/`REDO-TOTAL` drive at session
/// end) — a clean session must trip nothing.
#[test]
fn undo_redo_total_clean_script_via_driver_trips_nothing() {
    let content = "seed content\n";
    let script = vec![
        Action::Type("abc".to_string()),
        Action::Key(key(KeyCode::Left, Mods::NONE)),
        Action::Type("xyz".to_string()),
        Action::Key(key(KeyCode::Backspace, Mods::NONE)),
    ];
    let result = driver::run(content, &script);
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

#[test]
fn undo_total_detects_content_mismatch_after_reaching_journal_pos_zero() {
    let mut after = base_snapshot("MUTATED, not the seed");
    after.journal_pos = 0;
    after.journal_len = 3;
    let v = undo_total("original seed content", &after)
        .expect("journal_pos==0 but content != seed must trip UNDO-TOTAL");
    assert_eq!(v.id, "UNDO-TOTAL");
}

#[test]
fn undo_total_detects_bound_exhaustion() {
    let mut after = base_snapshot("original seed content");
    after.journal_pos = 2; // never reached 0 within the bound
    after.journal_len = 3;
    let v = undo_total("original seed content", &after)
        .expect("failing to reach journal_pos==0 within the bound must trip UNDO-TOTAL");
    assert_eq!(v.id, "UNDO-TOTAL");
}

#[test]
fn undo_total_accepts_converged_matching_content() {
    let mut after = base_snapshot("original seed content");
    after.journal_pos = 0;
    after.journal_len = 3;
    assert_eq!(undo_total("original seed content", &after), None);
}

#[test]
fn redo_total_detects_content_mismatch_after_reaching_the_target_pos() {
    let mut pre_undo = base_snapshot("the fully-edited content");
    pre_undo.journal_pos = 3; // the pos the session was actually at
    pre_undo.journal_len = 3;
    let mut after = base_snapshot("WRONG, not restored");
    after.journal_pos = 3; // reached the target pos...
    after.journal_len = 3;
    let v = redo_total(&pre_undo, &after).expect(
        "journal_pos==pre_undo.journal_pos but content != pre-undo content must trip REDO-TOTAL",
    );
    assert_eq!(v.id, "REDO-TOTAL");
}

#[test]
fn redo_total_detects_bound_exhaustion() {
    let mut pre_undo = base_snapshot("the fully-edited content");
    pre_undo.journal_pos = 3; // the pos the session was actually at
    pre_undo.journal_len = 3;
    let mut after = base_snapshot("the fully-edited content");
    after.journal_pos = 1; // never reached pre_undo.journal_pos within the bound
    after.journal_len = 3;
    let v = redo_total(&pre_undo, &after).expect(
        "failing to reach journal_pos==pre_undo.journal_pos within the bound must trip \
         REDO-TOTAL",
    );
    assert_eq!(v.id, "REDO-TOTAL");
}

#[test]
fn redo_total_accepts_converged_matching_content() {
    let mut pre_undo = base_snapshot("the fully-edited content");
    pre_undo.journal_pos = 3;
    pre_undo.journal_len = 3;
    let mut after = base_snapshot("the fully-edited content");
    after.journal_pos = 3;
    after.journal_len = 3;
    assert_eq!(redo_total(&pre_undo, &after), None);
}

/// Regression pin: the session's OWN last action can legitimately be an
/// undo, leaving `journal_pos < journal_len` with an intact redo tail when
/// the end-of-session drive begins. `REDO-TOTAL`'s target must be
/// `pre_undo.journal_pos` (here `2`), NOT `journal_len` (`5`) — driving
/// redo past `2` would walk past where the session actually left off.
#[test]
fn redo_total_accepts_a_mid_history_target_not_the_full_tip() {
    let mut pre_undo = base_snapshot("hel"); // session ended mid-history
    pre_undo.journal_pos = 2;
    pre_undo.journal_len = 5; // a live redo tail (3 more steps) exists
    let mut after = base_snapshot("hel"); // redo drive stopped back at the same pos
    after.journal_pos = 2;
    after.journal_len = 5;
    assert_eq!(redo_total(&pre_undo, &after), None);
}
