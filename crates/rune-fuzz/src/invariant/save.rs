//! `SAVE-VERBATIM`/`SAVE-CLEAN-MATCHES-DISK` (§1.4.5 byte-verbatim writes;
//! §1.4.8 durable-publish-then-clean ordering) — both need `StepCtx`'s VFS
//! read and save-delivery bookkeeping, which `Snapshot` structurally can't
//! hold (plan Context, decision 7 `[fixes B3]`).

use super::{Violation, trunc};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `SAVE-VERBATIM` (§1.4.5; Go's fuzzer names the same invariant
/// `SAVE-VERBATIM`) — on a successful `SaveDone`, the bytes actually on
/// disk must byte-equal the bytes THAT save was constructed with. Byte
/// comparison, no normalization.
///
/// Active-document-switch-safe: takes only `ctx`. `disk` is bound to the
/// fixed path the session was seeded with (`driver.rs`'s `State::path`),
/// never to whichever document is currently active. `delivered_save_bytes`
/// is doc-scoped too, but NOT by construction the way this comment used to
/// claim — the driver looks it up by the `SaveDone` ack's own `id` (see
/// `MsgTag::SaveDone`'s docs), precisely because a naive "whatever's
/// active" capture is exactly what TODO-fuzz-save-verbatim-help-doc-stale-
/// ack.md's repro broke: a Guard modal's own `s` hotkey can save a
/// document other than the active one, and the driver used to snapshot
/// `Snapshot::content` (the ACTIVE document) instead of the document the
/// save `Cmd` was actually constructed for.
pub fn save_verbatim(ctx: &StepCtx) -> Option<Violation> {
    if !matches!(ctx.msg, MsgTag::SaveDone { ok: true, .. }) {
        return None;
    }
    if ctx.disk.as_deref() == ctx.delivered_save_bytes.as_deref() {
        return None;
    }
    Some(Violation {
        id: "SAVE-VERBATIM",
        message: format!(
            "disk bytes do not byte-equal the delivered save bytes: disk={:?} delivered={:?}",
            ctx.disk
                .as_ref()
                .map(|b| trunc(&String::from_utf8_lossy(b), 80)),
            ctx.delivered_save_bytes
                .as_ref()
                .map(|b| trunc(&String::from_utf8_lossy(b), 80)),
        ),
    })
}

/// `SAVE-CLEAN-MATCHES-DISK` (§1.4.8) — once the document reports clean
/// (`!is_dirty`) with at least one successful save delivered and no save
/// still pending, disk bytes must byte-equal the current content. Catches
/// a save that reports success without actually persisting. Inert during
/// the `UNDO-TOTAL` drive (G5 keeps `is_dirty` true there — correct, not a
/// coverage hole).
///
/// `next.is_dirty`/`next.content` are doc-scoped (read off `app.active_
/// doc()` at capture time), but `ctx.disk` is NOT — it's `mem.read` against
/// the one fixed path the session was seeded and bound to (`driver.rs`'s
/// `State::path`), which only ever holds the real, seeded document's bytes.
/// `F1` (help toggle) swaps `app.active` to the virtual Help document,
/// whose synthetic markdown and trivial `is_dirty == false` have nothing to
/// do with that path — comparing them against `ctx.disk` misreports the
/// checker's own doc-vs-path mismatch as a durability defect (this is what
/// TODO-fuzz-save-clean-matches-disk-help-toggle.md's `Type("hello world")
/// -> ⌘S -> A -> F1 -> A` repro actually hit). Gated on `!next.read_only`
/// rather than the `prev.active == next.active` pattern `VERSION-MONOTONE`/
/// `SAVE-INFLIGHT-SM` use: this driver never opens more than one non-Help
/// document (`checks.rs::restore_editor_focus`'s own docs) and `read_only`
/// is set on exactly that one virtual document (`workspace::toggle_help`,
/// the only call site that ever sets it) — so "the active document is
/// read-only" and "the active document is not the one `ctx.disk` describes"
/// are the same condition here, without needing `StepCtx` to carry an
/// active document id at all.
pub fn save_clean_matches_disk(next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if next.read_only
        || next.is_dirty
        || ctx.saves_delivered_ok == 0
        || ctx.pending_save_bytes.is_some()
    {
        return None;
    }
    if ctx.disk.as_deref() == Some(next.content.as_bytes()) {
        return None;
    }
    Some(Violation {
        id: "SAVE-CLEAN-MATCHES-DISK",
        message: format!(
            "document reports clean but disk does not match content: disk={:?} content={:?}",
            ctx.disk
                .as_ref()
                .map(|b| trunc(&String::from_utf8_lossy(b), 80)),
            trunc(&next.content, 80)
        ),
    })
}
