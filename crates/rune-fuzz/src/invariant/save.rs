//! `SAVE-VERBATIM`/`SAVE-CLEAN-MATCHES-DISK` (byte-verbatim writes;
//! durable-publish-then-clean ordering) — both need `StepCtx`'s VFS
//! read and save-delivery bookkeeping, which `Snapshot` structurally can't
//! hold (plan Context, decision 7 `[fixes B3]`).

use super::{Violation, trunc};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `SAVE-VERBATIM` — on a successful `SaveDone`, the bytes actually on
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
    Some(Violation::new(
        "SAVE-VERBATIM",
        format!(
            "disk bytes do not byte-equal the delivered save bytes: disk={:?} delivered={:?}",
            ctx.disk
                .as_ref()
                .map(|b| trunc(&String::from_utf8_lossy(b), 80)),
            ctx.delivered_save_bytes
                .as_ref()
                .map(|b| trunc(&String::from_utf8_lossy(b), 80)),
        ),
    ))
}

pub fn save_clean_matches_disk(next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !ctx.active_is_seed_doc
        || next.is_dirty
        || ctx.saves_delivered_ok == 0
        || ctx.pending_save_bytes.is_some()
        || ctx.disk_diverged_since_publish
    {
        return None;
    }
    if ctx.disk.as_deref() == Some(next.content.as_bytes()) {
        return None;
    }
    Some(Violation::new(
        "SAVE-CLEAN-MATCHES-DISK",
        format!(
            "document reports clean but disk does not match content: disk={:?} content={:?}",
            ctx.disk
                .as_ref()
                .map(|b| trunc(&String::from_utf8_lossy(b), 80)),
            trunc(&next.content, 80)
        ),
    ))
}

pub fn save_no_trailing_ws(next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !ctx.active_is_seed_doc
        || next.is_dirty
        || ctx.saves_delivered_ok == 0
        || ctx.pending_save_bytes.is_some()
        || ctx.disk_diverged_since_publish
    {
        return None;
    }
    let (line_number, line) = trailing_whitespace_line(ctx.disk.as_deref()?)?;
    Some(Violation::new(
        "SAVE-NO-TRAILING-WS",
        format!(
            "line {line_number} on disk ends in whitespace: {}",
            visible(&line)
        ),
    ))
}

fn trailing_whitespace_line(disk: &[u8]) -> Option<(usize, Vec<u8>)> {
    disk.split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .enumerate()
        .find(|(_, line)| matches!(line.last(), Some(b'\t' | b' ')))
        .map(|(index, line)| (index + 1, line.to_vec()))
}

fn visible(line: &[u8]) -> String {
    let run = line
        .iter()
        .rev()
        .take_while(|byte| matches!(byte, b'\t' | b' '))
        .count();
    let head_len = line.len().saturating_sub(run);
    let head = line.get(..head_len).unwrap_or(line);
    let mut out = trunc(
        &String::from_utf8_lossy(head).escape_debug().to_string(),
        60,
    );
    for byte in line.iter().skip(head_len) {
        out.push_str(if *byte == b'\t' { "<TAB>" } else { "<SP>" });
    }
    out
}
