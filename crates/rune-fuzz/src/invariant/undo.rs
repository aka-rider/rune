//! Undo/redo invariants: `REDO-CLEAR` fires per step; `UNDO-TOTAL`/
//! `REDO-TOTAL` ("the trust test": load -> N operations -> undo ALL ->
//! byte-identical to original) run once, at session end, after
//! `driver.rs` drives the actual undo/redo presses.
//!
//! `UNDO-TOTAL` compares CONTENT ONLY (G5: `apply_edits` bumps `Buffer::
//! version()` on every call including inverses, so undoing to `journal_pos
//! == 0` leaves `is_dirty()` true even though content matches the seed —
//! that is intended, not a bug). Bound-exhaustion is folded into the same
//! checker via `Snapshot.journal_pos`: `driver.rs` stops driving after
//! `journal_len + 8` presses, and if that didn't reach the target
//! position, `after.journal_pos` still shows it — a non-converging
//! undo/redo is itself a violation, not a silent no-op.
//!
//! Both drives assume `⌘Z`/`⌘⇧Z` actually reach the SEEDED document — they
//! don't unless it is the active one, the editor is focused, and no modal
//! owns the keyboard. `driver/checks.rs::restore_editor_focus` establishes
//! that precondition with real keystrokes immediately before either drive
//! starts, so a session that ends with (say) the Explorer focused or an
//! untitled draft active no longer misreads "the keys went somewhere else"
//! as "undo doesn't converge".

use super::{Violation, trunc};
use crate::snapshot::Snapshot;

/// `REDO-CLEAR` (L1, every step) — whenever a step both bumps the version
/// AND pushes a new journal step (a real edit landed, not an undo/redo
/// `move_pos`), the journal's redo tail must already be gone:
/// `journal_pos == journal_len`. `Journal::push` truncates the tail before
/// pushing (`rune-core/src/undo.rs`), so this can only fail if some path
/// pushed a step without going through it. Scoped to `prev.active ==
/// next.active` for the same reason `PANE-NO-BLEED`/`VERSION-MONOTONE` are:
/// switching the active document (`F1` toggling to/from the Help virtual
/// document) makes a "version went up, journal grew" comparison meaningless
/// — it's two different documents' journals, not one growing.
pub fn redo_clear(prev: &Snapshot, next: &Snapshot) -> Option<Violation> {
    if prev.active != next.active {
        return None;
    }
    if next.version > prev.version
        && next.journal_len > prev.journal_len
        && next.journal_pos != next.journal_len
    {
        return Some(Violation::new(
            "REDO-CLEAR",
            format!(
                "a new edit landed (journal_len {} -> {}) but journal_pos={} != journal_len",
                prev.journal_len, next.journal_len, next.journal_pos
            ),
        ));
    }
    None
}

/// `UNDO-TOTAL` (end-of-session, once) — `seed_content` is the ORIGINAL
/// content `driver::run` was seeded with (NOT the state right before the
/// undo drive began — that edited state is what `redo_total` restores to,
/// a different comparison); `after` is the state once the drive stopped
/// (either `journal_pos == 0` or the `journal_len + 8` bound was reached).
/// Content-only per G5.
///
/// `after` describes whatever document was ACTIVE when the drive stopped,
/// so this comparison is only meaningful while that document is the seeded
/// one. Which it is, is a driver PRECONDITION this checker cannot detect
/// or correct for itself: `driver/checks.rs::restore_editor_focus` presses
/// the seed's own `^1`-`^0` tab chord to put it back under the keyboard,
/// and `drive_end_of_session_checks` refuses to call this function at all
/// unless the seed ended up active — including when it was discarded
/// outright by a dirty-close Guard's `[D]iscard` and there is no tab left
/// to switch to.
pub fn undo_total(seed_content: &str, after: &Snapshot) -> Option<Violation> {
    if after.journal_pos != 0 {
        return Some(Violation::new(
            "UNDO-TOTAL",
            format!(
                "undo did not converge to journal_pos == 0 within the bound (stuck at pos={} \
                 of len={})",
                after.journal_pos, after.journal_len
            ),
        ));
    }
    if after.content != seed_content {
        return Some(Violation::new(
            "UNDO-TOTAL",
            format!(
                "content after undoing to journal_pos == 0 does not match the seed: seed={:?} \
                 after={:?}",
                trunc(seed_content, 80),
                trunc(&after.content, 80)
            ),
        ));
    }
    None
}

/// `REDO-TOTAL` (end-of-session, once, immediately after `UNDO-TOTAL`'s
/// drive reached `journal_pos == 0`) — `pre_undo` is the state captured
/// right before the `UNDO-TOTAL` drive began; `after` is the state once
/// the redo drive stopped. The redo target is `pre_undo.journal_pos`, NOT
/// unconditionally `journal_len`: a session can legitimately end with its
/// OWN last action being an undo (or several), leaving `journal_pos <
/// journal_len` with an intact, never-superseded redo tail — driving redo
/// past that point would walk PAST where the session actually left off
/// and assert content against a state the session was never in. Undo `k`
/// times then redo `k` times must return to exactly where you started,
/// for whatever `k` (and whatever starting `journal_pos`) the session
/// happened to have; that symmetric round-trip is what this actually
/// checks.
///
/// Both snapshots describe the same document: neither drive's own keys
/// (`⌘Z`/`⌘⇧Z`) switch `app.active`, and the driver has already pinned the
/// seeded document as the active one before `pre_undo` was captured — a
/// precondition, not something this checker asserts, see `undo_total`'s
/// docs.
pub fn redo_total(pre_undo: &Snapshot, after: &Snapshot) -> Option<Violation> {
    if after.journal_pos != pre_undo.journal_pos {
        return Some(Violation::new(
            "REDO-TOTAL",
            format!(
                "redo did not converge back to the pre-undo-drive journal_pos={} within the \
                 bound (stuck at pos={} of len={})",
                pre_undo.journal_pos, after.journal_pos, after.journal_len
            ),
        ));
    }
    if after.content != pre_undo.content {
        return Some(Violation::new(
            "REDO-TOTAL",
            format!(
                "content after redoing back to the pre-undo-drive journal_pos does not match \
                 the pre-undo-drive content: pre_undo={:?} after={:?}",
                trunc(&pre_undo.content, 80),
                trunc(&after.content, 80)
            ),
        ));
    }
    None
}
