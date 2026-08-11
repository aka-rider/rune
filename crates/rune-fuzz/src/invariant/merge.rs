//! Merge mode invariants (plan WP7.S1) — four named checks over the lean
//! `merge_*`/`display_name_by_doc`/`scroll_row` projection `Snapshot`
//! carries (module docs there): `MERGE-DOC-ACTIVE`, `MERGE-SAVE-BLOCKED`,
//! `MERGE-KEY-FEEDBACK`, `MERGE-TITLE-CLEARED` — plus the stateful
//! `MERGE-NO-INSTANT-REDIVERGENCE` and `SAVE-AGREES-WITH-DIVERGENCE`
//! trackers, driven per step by `driver::step_and_check` rather than
//! `check_all`'s pure fold.

use rune_db::{DbEvent, OpOutcome, SyncKind};
use rune_tui::document::DocumentId;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::Command;
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
        return Some(Violation {
            id: "MERGE-DOC-ACTIVE",
            message: "merge is Active but MergeState::doc() is None".to_string(),
        });
    };
    if !next.dirty_by_doc.contains_key(&doc) {
        return Some(Violation {
            id: "MERGE-DOC-ACTIVE",
            message: format!("merge is Active for {doc:?} but that document is no longer open"),
        });
    }
    if next.active != doc {
        return Some(Violation {
            id: "MERGE-DOC-ACTIVE",
            message: format!(
                "merge is Active for {doc:?} but the active document is {:?}",
                next.active
            ),
        });
    }
    None
}

/// `MERGE-SAVE-BLOCKED` (plan WP4.S3's save gate) — a `Command::Save` key
/// pressed while merge is `Active` with unresolved blocks must never arm a
/// save: no `Cmd` gets constructed (`ctx.pending_save_bytes` stays `None`)
/// and `save_in_flight` never flips on. Both are checked, not just one —
/// `pending_save_bytes` catches a save the driver never even delivered yet,
/// `save_in_flight` catches one the gate let slip through synchronously.
pub fn merge_save_blocked(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !prev.merge_active || prev.merge_unresolved == 0 {
        return None;
    }
    let is_save_key = matches!(
        ctx.msg,
        MsgTag::Key {
            command: Some(Command::Save),
            ..
        }
    );
    if !is_save_key {
        return None;
    }
    if ctx.pending_save_bytes.is_some() || next.save_in_flight {
        return Some(Violation {
            id: "MERGE-SAVE-BLOCKED",
            message: format!(
                "Command::Save scheduled a materialize while merge was Active with {} \
                 unresolved block(s) (pending_save_bytes={:?}, save_in_flight={})",
                prev.merge_unresolved, ctx.pending_save_bytes, next.save_in_flight
            ),
        });
    }
    None
}

/// `MERGE-KEY-FEEDBACK` (the House Rule `merge/keys.rs` module docs name:
/// "consuming with feedback, never silently") — the resolver owes feedback
/// for every key it REFUSES.
///
/// Scoped to `Pane::Editor` (plan `[R1]`): `merge/keys.rs::intercept`
/// itself is scoped to the active document, but a key delivered while some
/// OTHER pane (Explorer, Tabs, Title) is focused never reaches it at all —
/// that key's feedback obligation belongs to whichever pane's own table
/// resolves it, not to this invariant.
///
/// A bare or shift-only viewport key (`merge::keys::viewport_scroll`) is a
/// scroll request the resolver honours rather than refuses, and a scroll
/// that lands against the top or bottom of the working form is silent by
/// universal editor convention — so that one case is exempt from the "left
/// an observable trace" demand below, but ONLY when the merge document IS
/// the active one: when merge is `Active` on some OTHER document, the key
/// never reaches `intercept` at all, so a silent swallow there is still a
/// genuine defect and stays a violation. Every other key dispatched while
/// merge is `Active` and the Editor pane is focused must still leave an
/// observable trace: it changed the buffer (content/version), the cursor
/// set, the viewport scroll position, or merge state itself, or it set a
/// status message. `merge/keys.rs::intercept` is the sole owner of every key
/// in this state (it runs before the hardcoded Enter/Escape fast paths and
/// the printable-insert fallthrough), so this pins that its own fallback
/// arm — a `messages::warn(MERGE_KEY_HINT, ..)` post — is truly exhaustive
/// over everything it doesn't otherwise resolve.
pub fn merge_key_feedback(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !prev.merge_active || prev.focus != Pane::Editor {
        return None;
    }
    let MsgTag::Key { input, .. } = &ctx.msg else {
        return None;
    };
    if rune_tui::merge::keys::viewport_scroll(*input).is_some()
        && prev.merge_doc == Some(prev.active)
    {
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
    Some(Violation {
        id: "MERGE-KEY-FEEDBACK",
        message: format!(
            "{:?} was dispatched while merge was Active with Editor focus and left buffer, \
             cursors, scroll, merge state, status, and message log all unchanged",
            ctx.msg
        ),
    })
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
            return Some(Violation {
                id: "MERGE-NO-INSTANT-REDIVERGENCE",
                message: format!(
                    "{doc:?} re-classified Diverged on step {} ({:?}) with no external \
                     disk write since its merge completed — the re-merge-prompt loop",
                    ctx.step, ctx.msg
                ),
            });
        }
        None
    }
}

/// `SAVE-AGREES-WITH-DIVERGENCE` (issue #65) — a publish may never commit
/// while the document's own classification says the disk holds changes the
/// buffer does not, unless the user explicitly forced it. The
/// compare-and-swap alone cannot police this: it compares the live target
/// against the baseline this session last published, so a buffer undone
/// back behind a merge it once adopted still CAS-matches and would
/// overwrite the adopted changes. `SaveMode::Force` — the disk-conflict
/// Guard's own `[S]ave anyway` — is the one legitimate commit in that
/// state, and the mode itself never reaches a `Snapshot`, so it is inferred
/// from the Guard the answer dismisses.
///
/// Stateful across steps for the same reason
/// [`RedivergenceTracker`] is: the arming step (which is where both the
/// classification and the user's authorization are observable) and the
/// committing step are different steps.
#[derive(Debug, Default)]
pub struct DivergentSaveTracker {
    /// The document whose in-flight save was armed while its own
    /// classification said the disk was ahead, with no force authorization —
    /// `None` whenever no such attempt is outstanding.
    unauthorized: Option<DocumentId>,
}

impl DivergentSaveTracker {
    /// Feeds one checked step, in order, exactly like
    /// [`RedivergenceTracker::observe`].
    pub fn observe(
        &mut self,
        prev: &Snapshot,
        next: &Snapshot,
        ctx: &StepCtx,
    ) -> Option<Violation> {
        let doc = next.active;
        if !save_in_flight(prev, doc) && save_in_flight(next, doc) {
            let divergent = prev.active == doc
                && prev
                    .active_last_sync
                    .is_some_and(SyncKind::is_disk_divergent);
            let forced =
                matches!(&prev.guard, Some((guarded, GuardKind::DiskConflict)) if *guarded == doc);
            self.unauthorized = (divergent && !forced).then_some(doc);
            return None;
        }
        let watched = self.unauthorized?;
        if !save_in_flight(prev, watched) || save_in_flight(next, watched) {
            return None;
        }
        self.unauthorized = None;
        // `saved_version` is the active document's own — a save resolving
        // for some other document is invisible here, and correctly so: the
        // classification this tracker armed on is active-document-scoped
        // too.
        if next.active != watched || next.saved_version <= prev.saved_version {
            return None;
        }
        Some(Violation {
            id: "SAVE-AGREES-WITH-DIVERGENCE",
            message: format!(
                "{watched:?}'s save committed on step {} ({:?}) although its own classification \
                 said the disk held changes the buffer did not, and no force was authorized",
                ctx.step, ctx.msg
            ),
        })
    }
}

fn save_in_flight(snapshot: &Snapshot, doc: DocumentId) -> bool {
    snapshot
        .save_in_flight_by_doc
        .get(&doc)
        .copied()
        .unwrap_or(false)
}

/// `MERGE-THEIRS-CONFIRMED` (WP-A task 2ii/7): checked against the raw
/// `Msg` a `MergePrep` ack carries, before `handle_merge_prep_ack` ever
/// consumes it — the Snapshot/StepCtx projection has no visibility into an
/// observation's `confirmed` column, so this is driven directly by
/// `driver::step_exec` rather than folded into `check_all`, the same shape
/// `SAVE-SINGLE-FLIGHT` uses. `rune_db::merge_prep`'s own contract is that
/// `unstable: true` NEVER also carries a `theirs`/`theirs_obs` — a
/// persistently unconfirmed disk state is reported honestly, never served
/// as content to merge against. A violation here is a regression in that
/// contract, caught while the fuzzer is genuinely driving the store-backed
/// merge-prep op against `Mem`'s own fault injection.
pub fn merge_theirs_confirmed(msg: &Msg) -> Option<Violation> {
    let Msg::Db(DbEvent::Ok {
        result: OpOutcome::MergePrep(prep),
        ..
    }) = msg
    else {
        return None;
    };
    if prep.unstable && (prep.theirs.is_some() || prep.theirs_obs.is_some()) {
        return Some(Violation {
            id: "MERGE-THEIRS-CONFIRMED",
            message: "a MergePrep ack reported unstable=true but still carried a theirs/\
                       theirs_obs — an unconfirmed observation must never be rendered as \
                       merge Theirs"
                .to_string(),
        });
    }
    None
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
            return Some(Violation {
                id: "MERGE-TITLE-CLEARED",
                message: format!("merge is Inactive but {doc:?}'s display_name is still {name:?}"),
            });
        }
    }
    None
}
