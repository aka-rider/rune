//! Merge mode invariants (plan WP7.S1) — four named checks over the lean
//! `merge_*`/`display_name_by_doc`/`scroll_row` projection `Snapshot`
//! carries (module docs there): `MERGE-DOC-ACTIVE`, `MERGE-SAVE-BLOCKED`,
//! `MERGE-KEY-FEEDBACK`, `MERGE-TITLE-CLEARED`.

use rune_tui::keymap::Command;
use rune_tui::pane::Pane;

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
/// "consuming with feedback, never silently") — any key dispatched while
/// merge is `Active` and the Editor pane is focused must leave an
/// observable trace: it changed the buffer (content/version), the cursor
/// set, the viewport scroll position, or merge state itself, or it set a
/// status message. `merge/keys.rs::intercept` is the sole owner of every
/// key in this state (it runs before the hardcoded Enter/Escape fast paths
/// and the printable-insert fallthrough), so this pins that its own
/// fallback arm — a `messages::warn(MERGE_KEY_HINT, ..)` post — is truly
/// exhaustive over everything it doesn't otherwise resolve.
///
/// Scoped to `Pane::Editor` (plan `[R1]`): `merge/keys.rs::intercept`
/// itself is scoped to the active document, but a key delivered while some
/// OTHER pane (Explorer, Tabs, Title) is focused never reaches it at all —
/// that key's feedback obligation belongs to whichever pane's own table
/// resolves it, not to this invariant.
pub fn merge_key_feedback(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !prev.merge_active || prev.focus != Pane::Editor {
        return None;
    }
    if !matches!(ctx.msg, MsgTag::Key { .. }) {
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
    if buffer_changed || cursors_changed || scroll_changed || merge_state_changed || status_changed
    {
        return None;
    }
    Some(Violation {
        id: "MERGE-KEY-FEEDBACK",
        message: format!(
            "{:?} was dispatched while merge was Active with Editor focus and left buffer, \
             cursors, scroll, merge state, and status all unchanged",
            ctx.msg
        ),
    })
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
