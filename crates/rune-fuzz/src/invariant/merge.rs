//! Merge mode invariants (plan WP7.S1) — four named checks over the lean
//! `merge_*`/`display_name_by_doc`/`scroll_row` projection `Snapshot`
//! carries (module docs there): `MERGE-DOC-ACTIVE`, `MERGE-SAVE-BLOCKED`,
//! `MERGE-KEY-FEEDBACK`, `MERGE-TITLE-CLEARED` — plus the stateful
//! `MERGE-NO-INSTANT-REDIVERGENCE` and `SAVE-AGREES-WITH-DIVERGENCE`
//! trackers, driven per step by `driver::step_and_check` rather than
//! `check_all`'s pure fold.

use std::collections::BTreeMap;

use rune_db::{DbEvent, MaterializePrep, OpOutcome, SyncKind};
use rune_tui::document::DocumentId;
use rune_tui::focus::FocusTarget;
use rune_tui::guard::GuardKind;
use rune_tui::pane::Pane;
use rune_tui::runtime::Msg;

use super::Violation;
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `MERGE-DOC-ACTIVE` — whenever merge mode is `Active`, the document it
/// names must still be open AND must be the active document. `merge::
/// exit_in_place` is the chokepoint every transition away from that
/// (switch, close, quit) funnels through (plan WP6.S3); this is what would
/// catch a path that changed `app.active`/closed the merge document
/// without going through it.
pub fn merge_doc_active(next: &Snapshot) -> Option<Violation> {
    if !next.merge_active {
        return None;
    }
    let Some(doc) = next.merge_doc else {
        return Some(Violation::new(
            "MERGE-DOC-ACTIVE",
            "merge is Active but MergeState::doc() is None".to_string(),
        ));
    };
    if !next.dirty_by_doc.contains_key(&doc) {
        return Some(Violation::new(
            "MERGE-DOC-ACTIVE",
            format!("merge is Active for {doc:?} but that document is no longer open"),
        ));
    }
    if next.active != doc {
        return Some(Violation::new(
            "MERGE-DOC-ACTIVE",
            format!(
                "merge is Active for {doc:?} but the active document is {:?}",
                next.active
            ),
        ));
    }
    None
}

/// `MERGE-SAVE-BLOCKED` (plan WP4.S3's save gate) — no step may ARM a save
/// while a merge attempt owns the document: `Pending` (the disk-state round
/// trip that is about to install a resolver) as much as `Active` with
/// unresolved blocks.
///
/// The arming EDGE is what fires, never the standing state: a save armed
/// legitimately before the merge attempt began stays in flight across many
/// later steps, and reading `save_in_flight`/`pending_save_bytes` as
/// absolutes accuses every one of them. The trigger is that edge from ANY
/// route, not a `Command::Save` key — the guards' `[S]` answers and the quit
/// fan-out arm saves with no save key in sight, which is exactly where a
/// missing gate would hide.
pub fn merge_save_blocked(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    let merging = prev.merge_pending || (prev.merge_active && prev.merge_unresolved > 0);
    if !merging || prev.merge_doc != Some(prev.active) {
        return None;
    }
    let armed = (!prev.save_in_flight && next.save_in_flight) || ctx.save_newly_parked;
    if !armed {
        return None;
    }
    let state = if prev.merge_pending {
        "Pending".to_string()
    } else {
        format!("Active with {} unresolved block(s)", prev.merge_unresolved)
    };
    Some(Violation::new(
        "MERGE-SAVE-BLOCKED",
        format!(
            "{:?} armed a materialize while merge was {state} \
             (save_newly_parked={}, save_in_flight={})",
            ctx.msg, ctx.save_newly_parked, next.save_in_flight
        ),
    ))
}

/// `MERGE-KEY-FEEDBACK` — the diff verb layer owes feedback for every key
/// it OWNS while merge mode is active on the focused document: a
/// `DIFF_BINDINGS` chord or the bare-Escape exit must always leave an
/// observable trace. Every other key falls through to ordinary editor
/// dispatch in the pane front-end (there is no total capture any more), so
/// its silence conventions belong to the editor, not to merge mode.
///
/// Scoped to `Pane::Editor` (plan `[R1]`): a key delivered while some
/// OTHER pane (Explorer, Tabs, Title) is focused never reaches the diff
/// intercept at all — that key's feedback obligation belongs to whichever
/// pane's own table resolves it, not to this invariant.
pub fn merge_key_feedback(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !prev.merge_active || prev.focus != Pane::Editor || prev.focus_target != FocusTarget::Editor
    {
        return None;
    }
    let MsgTag::Key { input, .. } = &ctx.msg else {
        return None;
    };
    let bare_escape = input.code == rune_tui::keymap::KeyCode::Escape
        && input.mods == rune_tui::keymap::Mods::NONE;
    let owned = bare_escape
        || rune_tui::binding::resolve_in(rune_tui::diff_view::keys::DIFF_BINDINGS, *input)
            .is_some();
    if !owned || prev.merge_doc != Some(prev.active) {
        return None;
    }
    let buffer_changed = prev.content != next.content || prev.version != next.version;
    let cursors_changed = prev.cursors != next.cursors;
    let scroll_changed = prev.scroll_row != next.scroll_row;
    let merge_state_changed = prev.merge_active != next.merge_active
        || prev.merge_pending != next.merge_pending
        || prev.merge_doc != next.merge_doc
        || prev.merge_unresolved != next.merge_unresolved;
    let status_changed = prev.status != next.status;
    // `status` alone is blind to two consecutive posts of IDENTICAL text
    // (the same merge-key hint fired by two different unbound keys in a
    // row): `entries.last()` looks unchanged even though a new row landed
    // in the log. `message_posts` is `messages::post`'s own monotonic
    // counter, so it always distinguishes "nothing was posted" from
    // "a duplicate was posted".
    let message_posted = prev.message_posts != next.message_posts;
    if buffer_changed
        || cursors_changed
        || scroll_changed
        || merge_state_changed
        || status_changed
        || message_posted
    {
        return None;
    }
    Some(Violation::new(
        "MERGE-KEY-FEEDBACK",
        format!(
            "{:?} was dispatched while merge was Active with Editor focus and left buffer, \
             cursors, scroll, merge state, status, and message log all unchanged",
            ctx.msg
        ),
    ))
}

/// `MERGE-NO-INSTANT-REDIVERGENCE` — the anti-loop invariant. When a merge
/// attempt retires (`Active`/`Pending` → fully `Inactive`) leaving its
/// document on a reconciled classification (`BufferAhead`/`Clean`), no
/// later step may re-classify that same document `Diverged` unless
/// something genuinely moved underneath it again: an external disk write
/// (the driver reports those through [`RedivergenceTracker::
/// note_external_write`] — `Action::DivergeDisk` runs outside the
/// step/checker cycle), an undo unwinding the buffer past the
/// reconciliation (a `journal_pos` decrease on the tracked document), or a
/// trash/rename changing the document's disk identity. A `Diverged` that
/// appears with none of those is the re-merge-prompt loop: a probe or a
/// save-time CAS refusal fabricating divergence for a document the merge
/// just reconciled.
///
/// Stateful across steps — the completing transition and the offending
/// re-classification are different steps — so, like `SAVE-SINGLE-FLIGHT`,
/// it lives on the driver and is fed every checked step in order instead of
/// being listed in `check_all`.
#[derive(Debug, Default)]
pub struct RedivergenceTracker {
    /// The document a retired merge attempt left reconciled, while nothing
    /// has legitimately moved it since. `None` = nothing to police.
    reconciled_doc: Option<DocumentId>,
}

impl RedivergenceTracker {
    /// The driver just rewrote the document's file behind the session's
    /// back — any later `Diverged` is truthful again.
    pub fn note_external_write(&mut self) {
        self.reconciled_doc = None;
    }

    /// Feeds one checked step. Must see every step, in order, whether or
    /// not any other checker fired first — the driver stops the session on
    /// a violation anyway, so ordering against `check_all` is free.
    pub fn observe(
        &mut self,
        prev: &Snapshot,
        next: &Snapshot,
        ctx: &StepCtx,
    ) -> Option<Violation> {
        if matches!(ctx.msg, MsgTag::TrashDone | MsgTag::RenameDone) {
            self.reconciled_doc = None;
            return None;
        }
        if let Some(doc) = self.reconciled_doc
            && prev.active == doc
            && next.active == doc
            && next.journal_pos < prev.journal_pos
        {
            self.reconciled_doc = None;
            return None;
        }
        let merge_retired =
            (prev.merge_active || prev.merge_pending) && !next.merge_active && !next.merge_pending;
        if merge_retired && prev.merge_doc == Some(next.active) {
            self.reconciled_doc = match next.active_last_sync {
                Some(SyncKind::BufferAhead) | Some(SyncKind::Clean) => prev.merge_doc,
                // A retirement still classified `DiskAhead`/`Diverged` (an
                // Esc-out with unresolved blocks, a refused install) never
                // arms — the divergence is still real there.
                _ => None,
            };
            return None;
        }
        if let Some(doc) = self.reconciled_doc
            && next.active == doc
            && next.active_last_sync == Some(SyncKind::Diverged)
        {
            self.reconciled_doc = None;
            return Some(Violation::new(
                "MERGE-NO-INSTANT-REDIVERGENCE",
                format!(
                    "{doc:?} re-classified Diverged on step {} ({:?}) with no external \
                     disk write since its merge completed — the re-merge-prompt loop",
                    ctx.step, ctx.msg
                ),
            ));
        }
        None
    }
}

/// `SAVE-AGREES-WITH-DIVERGENCE` (issue #65) — a publish may never commit
/// once the store's own prepare-time verdict said the disk holds changes
/// the buffer does not, unless the user explicitly forced it.
///
/// The verdict comes from the `MaterializePrepare` ack itself — the exact
/// value the production gate decides on — read off the raw `Msg` before
/// `update` consumes it.
/// `SaveMode::Force` never reaches a `Snapshot`, so authorization is taken
/// at the step the save arms, where the disk-conflict Guard the `[S]ave
/// anyway` answer dismisses is still up.
#[derive(Debug, Default)]
pub struct DivergentSaveTracker {
    attempts: BTreeMap<DocumentId, Attempt>,
}

#[derive(Debug)]
struct Attempt {
    forced: bool,
    divergent_verdict_step: Option<usize>,
}

impl DivergentSaveTracker {
    /// Reads the prepare ack `doc`'s in-flight save is waiting on, before
    /// `update` delivers it to the gate. `doc` is the document the pending
    /// op was recorded for.
    pub fn note_prepare_ack(&mut self, msg: &Msg, doc: Option<DocumentId>, step: usize) {
        let Msg::Db(DbEvent::Ok {
            result: OpOutcome::MaterializePrep(prep),
            ..
        }) = msg
        else {
            return;
        };
        let divergent = matches!(
            prep.as_ref(),
            MaterializePrep::Overwrite { sync, .. } if sync.is_disk_divergent()
        );
        if !divergent {
            return;
        }
        let Some(attempt) = doc.and_then(|doc| self.attempts.get_mut(&doc)) else {
            return;
        };
        attempt.divergent_verdict_step = Some(step);
    }

    /// Feeds one checked step, in order, exactly like
    /// [`RedivergenceTracker::observe`].
    pub fn observe(
        &mut self,
        prev: &Snapshot,
        next: &Snapshot,
        ctx: &StepCtx,
    ) -> Option<Violation> {
        for (&doc, &in_flight) in &next.save_in_flight_by_doc {
            if in_flight && !save_in_flight(prev, doc) {
                let forced = matches!(
                    &prev.guard,
                    Some((guarded, GuardKind::DiskConflict)) if *guarded == doc
                );
                self.attempts.insert(
                    doc,
                    Attempt {
                        forced,
                        divergent_verdict_step: None,
                    },
                );
            }
        }
        let mut violation = None;
        self.attempts.retain(|&doc, attempt| {
            if save_in_flight(next, doc) {
                return true;
            }
            let committed = saved_version(next, doc) > saved_version(prev, doc);
            let Some(verdict_step) = attempt.divergent_verdict_step else {
                return false;
            };
            if attempt.forced || !committed || violation.is_some() {
                return false;
            }
            violation = Some(Violation::new(
                "SAVE-AGREES-WITH-DIVERGENCE",
                format!(
                    "{doc:?}'s save committed on step {} ({:?}) although the prepare ack it \
                     published on (step {verdict_step}) carried a disk-divergent verdict, and no \
                     force was authorized",
                    ctx.step, ctx.msg
                ),
            ));
            false
        });
        violation
    }
}

fn save_in_flight(snapshot: &Snapshot, doc: DocumentId) -> bool {
    snapshot
        .save_in_flight_by_doc
        .get(&doc)
        .copied()
        .unwrap_or(false)
}

fn saved_version(snapshot: &Snapshot, doc: DocumentId) -> u64 {
    snapshot
        .saved_version_by_doc
        .get(&doc)
        .copied()
        .unwrap_or(0)
}

/// `MERGE-TITLE-CLEARED` — once merge mode is fully `Inactive` (neither
/// `Active` nor `Pending`), no open document's `display_name` may still
/// read the merge retitle (`merge::landing`'s `"{file_name}: editor <->
/// disk"`). `merge::exit_in_place` is the sole restorer of
/// `saved_display_name` (plan WP3.S7); this is what would catch a path
/// that cleared `app.merge` without going through it.
pub fn merge_title_cleared(next: &Snapshot) -> Option<Violation> {
    if next.merge_active || next.merge_pending {
        return None;
    }
    for (doc, name) in &next.display_name_by_doc {
        if name
            .as_deref()
            .is_some_and(|n| n.contains("editor <-> disk"))
        {
            return Some(Violation::new(
                "MERGE-TITLE-CLEARED",
                format!("merge is Inactive but {doc:?}'s display_name is still {name:?}"),
            ));
        }
    }
    None
}

/// `MERGE-UNDO-NEVER-COMPLETES` (issue #106) — a step that moves the
/// journal BACKWARD may retire an `Active` merge (the working form itself
/// was unwound), but it may never present that retirement as a completion:
/// re-classifying the document reconciled (`BufferAhead`/`Clean`) on the
/// same step is `exit_in_place`'s terminal-success arm firing on an undo,
/// which also advances the save-CAS baseline toward a buffer the user never
/// resolved. Completion stays a user act; an undo only ever abandons.
pub fn merge_undo_never_completes(prev: &Snapshot, next: &Snapshot) -> Option<Violation> {
    if !prev.merge_active || next.merge_active {
        return None;
    }
    if prev.merge_doc != Some(prev.active) || next.active != prev.active {
        return None;
    }
    if next.journal_pos >= prev.journal_pos {
        return None;
    }
    let reclassified_reconciled = matches!(
        next.active_last_sync,
        Some(SyncKind::BufferAhead) | Some(SyncKind::Clean)
    ) && next.active_last_sync != prev.active_last_sync;
    if !reclassified_reconciled {
        return None;
    }
    Some(Violation::new(
        "MERGE-UNDO-NEVER-COMPLETES",
        format!(
            "{:?}'s merge retired as reconciled ({:?}) on a journal-backward step \
             ({} -> {}) — an undo completed a merge the user never resolved",
            prev.active, next.active_last_sync, prev.journal_pos, next.journal_pos
        ),
    ))
}
