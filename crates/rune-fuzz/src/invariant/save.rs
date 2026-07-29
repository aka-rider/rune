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
pub fn save_clean_matches_disk(next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if next.is_dirty || ctx.saves_delivered_ok == 0 || ctx.pending_save_bytes.is_some() {
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
