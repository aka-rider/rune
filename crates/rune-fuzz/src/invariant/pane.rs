//! `PANE-NO-BLEED` — the rule the `UNDO-TOTAL`/`REDO-TOTAL` harness fix
//! (`driver.rs::restore_editor_focus`) rests on, pinned as an invariant so
//! a future change toward Go's behaviour (`workspace_update_keys.go`
//! Priority 2.5 bleeding an unfocused pane's keystroke into the document
//! as an invisible edit) is caught by the fuzzer instead of landing
//! silently. Needs `StepCtx.msg` (L2): a keystroke aimed at chrome — the
//! Explorer or the Open Tabs pane, with no modal capturing it first — must
//! never mutate the active document behind it.

use rune_tui::pane::Pane;

use super::{Violation, trunc};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// Fires when, on a `MsgTag::Key` step, `prev` had no modal up, focus
/// somewhere other than `Pane::Editor`, and the SAME active document
/// before and after — yet that document's `content`/`version`/
/// `journal_len` changed anyway.
///
/// The `prev.active == next.active` guard is what keeps this free of
/// false positives: every non-editor key path that DOES change what a
/// `Snapshot` observes also changes the active document (Explorer `Enter`
/// opening a file via `workspace::open_path`; Tabs `Enter` via
/// `switch_to`; `^w` closing the active tab via `close_now`) — this
/// checker simply never fires on those, because it's scoped to the
/// no-active-document-change case. The paths that keep the active
/// document unchanged (`⌘S`, a focus/toggle chord, a quit chord, a failed
/// open raising `Modal::Error`, `^w` arming the Guard, closing a
/// non-active tab) never touch a buffer byte either, so they're silent
/// here too.
///
/// Scoped to `MsgTag::Key` deliberately: `Msg::Paste`/`Msg::ClipboardRead`
/// insert into `app.active` regardless of focus, and the driver
/// synthesizes `ClipboardRead` unprompted — async replies are out of this
/// invariant's domain, same reasoning as `clip_osc52`'s own module docs.
pub fn pane_no_bleed(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !matches!(ctx.msg, MsgTag::Key { .. }) {
        return None;
    }
    if prev.modal_open || prev.focus == Pane::Editor {
        return None;
    }
    if prev.active != next.active {
        return None;
    }
    let content_changed = prev.content != next.content;
    let version_changed = prev.version != next.version;
    let journal_changed = prev.journal_len != next.journal_len;
    if !content_changed && !version_changed && !journal_changed {
        return None;
    }
    Some(Violation {
        id: "PANE-NO-BLEED",
        message: format!(
            "a key aimed at {:?} (no modal up, active document unchanged) mutated the \
             document: content {:?} -> {:?}, version {} -> {}, journal_len {} -> {}",
            prev.focus,
            trunc(&prev.content, 80),
            trunc(&next.content, 80),
            prev.version,
            next.version,
            prev.journal_len,
            next.journal_len
        ),
    })
}
