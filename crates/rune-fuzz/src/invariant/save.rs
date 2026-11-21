//! `SAVE-VERBATIM`/`SAVE-CLEAN-MATCHES-DISK` (byte-verbatim writes;
//! durable-publish-then-clean ordering) — both need `StepCtx`'s VFS
//! read and save-delivery bookkeeping, which `Snapshot` structurally can't
//! hold (plan Context, decision 7 `[fixes B3]`).

use super::{Violation, trunc};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `SAVE-VERBATIM` — on a successful `SaveDone`, the file the save wrote
/// must byte-equal the bytes THAT save was constructed with. Byte
/// comparison, no normalization.
///
/// Takes only `ctx` and reads only `ctx.delivered_save`, which the driver
/// fills from the `SaveDone` ack's own document: the bytes and the disk
/// read of that document's own target path. It never reads `ctx.disk`
/// (the seed document's file), because a save for a draft or any other
/// document writes somewhere else. A `SaveDone` with no `delivered_save`
/// is itself a violation: the driver armed no save for that document.
pub fn save_verbatim(ctx: &StepCtx) -> Option<Violation> {
    if !matches!(ctx.msg, MsgTag::SaveDone { ok: true, .. }) {
        return None;
    }
    let Some(saved) = &ctx.delivered_save else {
        return Some(Violation::new(
            "SAVE-VERBATIM",
            "a successful SaveDone arrived for a document the driver armed no save for".to_string(),
        ));
    };
    if saved.disk_at_target.as_deref() == Some(saved.bytes.as_slice()) {
        return None;
    }
    Some(Violation::new(
        "SAVE-VERBATIM",
        format!(
            "disk bytes at the save target do not byte-equal the delivered save bytes: \
             disk={:?} delivered={:?}",
            saved
                .disk_at_target
                .as_ref()
                .map(|b| trunc(&String::from_utf8_lossy(b), 80)),
            trunc(&String::from_utf8_lossy(&saved.bytes), 80),
        ),
    ))
}

pub fn save_clean_matches_disk(next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !ctx.active_is_seed_doc
        || next.is_dirty
        || ctx.saves_delivered_ok == 0
        || ctx.save_parked
        || ctx.disk_diverged_since_publish
    {
        return None;
    }
    if ctx.disk.as_deref() == Some(rune_core::codec::encode(&next.content).as_ref()) {
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
        || ctx.save_parked
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
