//! Highlight overlay invariants: `HL-CLAMPED`, `HL-STALE-DROP`,
//! `HL-NO-REFLOW`. All three key off `Snapshot.highlight_spans` — the output
//! of `highlight::visible_spans`, the same query the renderer runs, with the
//! `ScopeId` tag dropped because no checker here needs it — and, for the L2
//! pair, `StepCtx.msg` being a `MsgTag::Highlighted`.
//!
//! Projecting the QUERY rather than the stored state is what keeps these
//! meaningful now that every code region is tree-backed: the stored span
//! channel is empty for most documents, so a projection reading it would
//! leave `HL-CLAMPED` and `HL-STALE-DROP` passing while testing nothing. It
//! also extends both to the whole-file path they never reached.

use super::{Violation, trunc};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `HL-CLAMPED` (L0) — every span the render query would paint satisfies
/// `start < end`, `end <= content.len()`, and both endpoints are `char`
/// boundaries.
///
/// Checked UNCONDITIONALLY, with no staleness escape. It used to be scoped
/// to `highlight_version == version`, because a reply was clamped once on
/// receipt and nothing re-clamped it afterwards — so an edit that shrank the
/// buffer left a stored span legitimately out of bounds (`[R2]`: stale
/// colours, never no colours), and checking unconditionally would have
/// flagged that expected staleness. The clamp now lives in the query itself,
/// which runs afresh against the live buffer every time anything reads it,
/// so a stale span is re-clamped rather than merely tolerated and there is
/// no case left where an out-of-bounds span is legitimate.
///
/// Active-document-switch-safe: L0, single `Snapshot`'s own
/// `highlight_spans`/`content`.
pub fn hl_clamped(next: &Snapshot) -> Option<Violation> {
    for &(start, end) in &next.highlight_spans {
        let in_bounds = end <= next.content.len();
        let ordered = start < end;
        let boundary =
            in_bounds && next.content.is_char_boundary(start) && next.content.is_char_boundary(end);
        if !ordered || !in_bounds || !boundary {
            return Some(Violation {
                id: "HL-CLAMPED",
                message: format!(
                    "stored highlight span {start}..{end} is out of bounds or off a char \
                     boundary in content of length {} ({:?})",
                    next.content.len(),
                    trunc(&next.content, 80)
                ),
            });
        }
    }
    None
}

/// `HL-STALE-DROP` (L2) — on a `MsgTag::Highlighted` step whose
/// `delivered_version` does not match the buffer version `next` observes
/// AFTER the step, the spans the render query yields must equal `prev`'s
/// exactly: a stale reply describes content the buffer has since moved past,
/// and `dispatch::handle_highlighted` must leave every region untouched
/// rather than adopting it.
///
/// Active-document-switch-safe: `dispatch::handle_highlighted` mutates only
/// the document named by `Msg::Highlighted { doc, .. }` (via `app.doc_mut`)
/// and never touches `app.active` itself, so `prev.active == next.active`
/// always holds across a `MsgTag::Highlighted` step — no explicit gate
/// needed.
pub fn hl_stale_drop(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    let MsgTag::Highlighted {
        delivered_version, ..
    } = ctx.msg
    else {
        return None;
    };
    if delivered_version == next.version {
        return None;
    }
    if prev.highlight_spans != next.highlight_spans {
        return Some(Violation {
            id: "HL-STALE-DROP",
            message: format!(
                "a Msg::Highlighted delivered at version {delivered_version} (live version {}) \
                 changed stored spans from {:?} to {:?} instead of leaving them untouched",
                next.version, prev.highlight_spans, next.highlight_spans
            ),
        });
    }
    None
}

/// `HL-NO-REFLOW` (L2) — a `MsgTag::Highlighted` step is a pure style
/// overlay (all tree-sitter output is a render-layer overlay); it must never
/// change `content`, `version`, `journal_pos`,
/// `journal_len`, or `is_dirty`, and — when cells were sampled on both
/// steps — must never change any rendered cell's `buf_offset` or `width`
/// (only `style` may differ).
///
/// Active-document-switch-safe: same reasoning as `hl_stale_drop` above —
/// `Msg::Highlighted` never touches `app.active`, so `prev`/`next` always
/// describe the same document across this step.
pub fn hl_no_reflow(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !matches!(ctx.msg, MsgTag::Highlighted { .. }) {
        return None;
    }
    if prev.content != next.content
        || prev.version != next.version
        || prev.journal_pos != next.journal_pos
        || prev.journal_len != next.journal_len
        || prev.is_dirty != next.is_dirty
    {
        return Some(Violation {
            id: "HL-NO-REFLOW",
            message: format!(
                "a Msg::Highlighted step changed document state: content {:?} -> {:?}, \
                 version {} -> {}, journal_pos {} -> {}, journal_len {} -> {}, \
                 is_dirty {} -> {}",
                trunc(&prev.content, 40),
                trunc(&next.content, 40),
                prev.version,
                next.version,
                prev.journal_pos,
                next.journal_pos,
                prev.journal_len,
                next.journal_len,
                prev.is_dirty,
                next.is_dirty
            ),
        });
    }
    if prev.cells.is_empty() || next.cells.is_empty() {
        return None;
    }
    if prev.cells.len() != next.cells.len() {
        return Some(Violation {
            id: "HL-NO-REFLOW",
            message: format!(
                "a Msg::Highlighted step changed the rendered row count: {} -> {}",
                prev.cells.len(),
                next.cells.len()
            ),
        });
    }
    for (r, (prow, nrow)) in prev.cells.iter().zip(next.cells.iter()).enumerate() {
        if prow.len() != nrow.len() {
            return Some(Violation {
                id: "HL-NO-REFLOW",
                message: format!(
                    "a Msg::Highlighted step changed row {r}'s cell count: {} -> {}",
                    prow.len(),
                    nrow.len()
                ),
            });
        }
        for (c, (pc, nc)) in prow.iter().zip(nrow.iter()).enumerate() {
            if pc.buf_offset != nc.buf_offset || pc.width != nc.width {
                return Some(Violation {
                    id: "HL-NO-REFLOW",
                    message: format!(
                        "a Msg::Highlighted step changed cell ({r},{c})'s geometry: \
                         buf_offset {} -> {}, width {} -> {}",
                        pc.buf_offset, nc.buf_offset, pc.width, nc.width
                    ),
                });
            }
        }
    }
    None
}
