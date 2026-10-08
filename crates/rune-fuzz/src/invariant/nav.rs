use rune_tui::document::ReadOnly;
use rune_tui::focus::FocusTarget;
use rune_tui::keymap::{Command, Extend, Motion};

use super::{Violation, trunc};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

pub fn nav_bounds(snap: &Snapshot) -> Option<Violation> {
    for &(doc, offset, on_char_boundary) in &snap.nav_places {
        let Some(&len) = snap.buffer_len_by_doc.get(&doc) else {
            continue;
        };
        if offset > len || !on_char_boundary {
            return Some(Violation::new(
                "NAV-BOUNDS",
                format!(
                    "nav place doc={doc:?} offset={offset} buffer len={len} \
                     on_char_boundary={on_char_boundary}"
                ),
            ));
        }
    }
    if snap.nav_current > snap.nav_places.len() {
        return Some(Violation::new(
            "NAV-BOUNDS",
            format!(
                "nav_current={} exceeds places.len()={}",
                snap.nav_current,
                snap.nav_places.len()
            ),
        ));
    }
    None
}

/// `NAV-PROGRESS` (L2) — a plain (non-extending) `LineDown`/`LineUp` that
/// reaches the editor moves the lone caret, unless it already sits where
/// vertical motion has nowhere to go: the end of the buffer for `LineDown`,
/// the first buffer line for `LineUp` (there `LineUp` hands focus to the
/// title instead of moving the caret).
///
/// Only checked when the key demonstrably dispatched to the editor's motion
/// command: the step is a `Key` that resolved to the motion, no Guard is
/// up, the editor (not an overlay or another pane) owns the keystroke, and
/// the document is editable (a read-only document scrolls instead of moving
/// the caret). Multi-cursor sets and extending variants are out of scope.
pub fn nav_progress(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    let MsgTag::Key {
        command: Some(Command::Motion(motion @ (Motion::LineDown | Motion::LineUp), Extend::No)),
        ..
    } = &ctx.msg
    else {
        return None;
    };
    if prev.modal_open
        || prev.focus_target != FocusTarget::Editor
        || prev.read_only != ReadOnly::No
        || prev.active != next.active
    {
        return None;
    }
    let ([before], [after]) = (prev.cursors.as_slice(), next.cursors.as_slice()) else {
        return None;
    };
    let from = before.position.get();
    let nowhere_to_go = match motion {
        Motion::LineDown => from == prev.content.len(),
        _ => prev
            .line_ends
            .first()
            .is_none_or(|&first_line_end| from <= first_line_end),
    };
    if nowhere_to_go || after.position.get() != from {
        return None;
    }
    Some(Violation::new(
        "NAV-PROGRESS",
        format!(
            "{motion:?} left the caret at {from} (buffer len {}, {} lines) though vertical \
             motion had somewhere to go; content={:?}",
            prev.content.len(),
            prev.line_count,
            trunc(&prev.content, 120)
        ),
    ))
}
