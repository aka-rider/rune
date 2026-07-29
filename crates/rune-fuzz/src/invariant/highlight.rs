//! Highlight overlay invariants (plan WP7.S7): `HL-CLAMPED`, `HL-STALE-
//! DROP`, `HL-NO-REFLOW`. All three key off `Snapshot.highlight_spans`
//! (`doc.highlight.spans`, `ScopeId` dropped — no checker here needs it)
//! and, for the L2 pair, `StepCtx.msg` being a `MsgTag::Highlighted`.

use super::{Violation, trunc};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `HL-CLAMPED` (L0) — whenever `highlight_version == version` (production's
/// OWN "spans still describe the live buffer" test — `highlight.rs::
/// schedule_highlight` uses the identical comparison), every stored
/// highlight range satisfies `start < end`, `end <= content.len()`, and
/// both endpoints are `char` boundaries.
///
/// Scoped to the version-matched case deliberately, not unconditionally:
/// `dispatch::handle_highlighted` clamps a reply's spans against the buffer
/// length ONLY at the moment it processes that reply: nothing re-clamps the
/// stored spans afterward. A later edit (including an undo) that shrinks
/// the buffer bumps `version` past `highlight_version` — a real, expected
/// state (WP5.S4's `[R2]`: "stale colours, never no colours" — the design
/// deliberately keeps stale spans in place rather than blanking them on
/// every keystroke) that the render layer's own window-clamped painter
/// (`render::overlay::apply_highlight_spans`) already tolerates safely.
/// Checking clamped-ness unconditionally would flag that expected staleness
/// as a violation; this is the fuzz session that first proved it (a `type`
/// into a fresh document, one `Action::Highlight` reply describing the
/// FULL typed content, then enough `⌘Z` presses to shrink the buffer back
/// past where the stored span ends).
///
/// Active-document-switch-safe: L0, single `Snapshot`'s own
/// `highlight_spans`/`highlight_version`/`content`/`version`.
pub fn hl_clamped(next: &Snapshot) -> Option<Violation> {
    if next.highlight_version != next.version {
        return None;
    }
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
/// AFTER the step, the stored spans must equal `prev`'s exactly: a stale
/// reply describes content the buffer has since moved past, and
/// `dispatch::handle_highlighted` must leave the previously stored spans
/// untouched rather than adopting it.
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
/// overlay (plan decision 1: "all tree-sitter output is a render-layer
/// overlay"); it must never change `content`, `version`, `journal_pos`,
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
