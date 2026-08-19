use std::ops::Range;

use rune_core::buffer::AppliedEdit;

use crate::app::App;
use crate::document::DocumentId;
use crate::messages;

use super::session::{BlockOrigin, Resolution};
use super::state::MergeState;

const GATE_OPEN_STATUS: &str = "all conflicts resolved — ^M or Esc completes the merge";

pub(crate) struct Delta {
    start: usize,
    old_len: usize,
    new_len: usize,
}

pub(crate) fn forward_deltas(edits: &[AppliedEdit]) -> Vec<Delta> {
    let mut deltas: Vec<Delta> = edits
        .iter()
        .map(|e| Delta {
            start: e.start,
            old_len: e.deleted.len(),
            new_len: e.insert.len(),
        })
        .collect();
    deltas.sort_by_key(|d| d.start);
    deltas
}

pub(crate) fn inverse_deltas(edits: &[AppliedEdit]) -> Vec<Delta> {
    let mut sorted: Vec<&AppliedEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| e.start);
    let mut cum: isize = 0;
    let mut deltas = Vec::with_capacity(sorted.len());
    for e in sorted {
        let start = (e.start as isize + cum).max(0) as usize;
        deltas.push(Delta {
            start,
            old_len: e.insert.len(),
            new_len: e.deleted.len(),
        });
        cum += e.deleted.len() as isize - e.insert.len() as isize;
    }
    deltas
}

fn apply_delta(range: &mut Range<usize>, delta: &Delta) -> bool {
    let delta_end = delta.start + delta.old_len;
    let diff = delta.new_len as isize - delta.old_len as isize;
    if delta_end <= range.start {
        range.start = (range.start as isize + diff).max(0) as usize;
        range.end = (range.end as isize + diff).max(0) as usize;
        false
    } else if delta.start >= range.end {
        false
    } else {
        range.start = range.start.min(delta.start);
        if delta_end <= range.end {
            range.end = (range.end as isize + diff).max(range.start as isize) as usize;
        } else {
            range.end = (delta.start + delta.new_len).max(range.start);
        }
        true
    }
}

fn remap(range: &mut Range<usize>, deltas: &[Delta]) -> bool {
    let mut hit = false;
    for delta in deltas {
        hit |= apply_delta(range, delta);
    }
    hit
}

pub(crate) fn remap_after_edit_batch(app: &mut App, id: DocumentId, applied: &[AppliedEdit]) {
    let MergeState::Active { doc, session } = &mut app.merge else {
        return;
    };
    if *doc != id {
        return;
    }
    let unresolved_before = session.unresolved_count();
    let deltas = forward_deltas(applied);
    for pair in &mut session.conflicts {
        if remap(&mut pair.block.range, &deltas) {
            pair.block.resolution = Resolution::HandEdited;
        }
    }
    if unresolved_before > 0 && session.unresolved_count() == 0 {
        messages::info(app, GATE_OPEN_STATUS);
    }
}

pub(crate) fn rederive_after_jump(app: &mut App, id: DocumentId, deltas: &[Delta]) {
    if app.merge.doc() != Some(id) {
        return;
    }
    let Some(document) = app.doc(id) else {
        return;
    };
    let journal_pos = document.journal.pos();
    let content = document.buffer.content().to_string();
    let MergeState::Active { session, .. } = &mut app.merge else {
        return;
    };
    if journal_pos < session.install_pos {
        let MergeState::Active { session, .. } = std::mem::take(&mut app.merge) else {
            return;
        };
        super::abandon_active(
            app,
            id,
            session.saved_display_name,
            "merge closed — undo removed the merged text; ^M to merge again",
        );
        return;
    }
    let unresolved_before = session.unresolved_count();
    let mut reopened = None;
    for (idx, pair) in session.conflicts.iter_mut().enumerate() {
        let was_resolved = pair.block.resolution.is_resolved();
        if !remap(&mut pair.block.range, deltas) {
            continue;
        }
        pair.block.range.end = pair.block.range.end.min(content.len());
        pair.block.range.start = pair.block.range.start.min(pair.block.range.end);
        pair.block.resolution = match content.get(pair.block.range.clone()) {
            None => Resolution::Unresolved,
            Some(bytes) if bytes == pair.conflict.theirs => Resolution::TookTheirs,
            Some(bytes) if bytes == pair.conflict.ours => match pair.block.origin {
                BlockOrigin::Conflict => Resolution::Unresolved,
                BlockOrigin::AutoApplied => Resolution::KeptOurs,
            },
            Some(_) => Resolution::HandEdited,
        };
        if was_resolved && !pair.block.resolution.is_resolved() && reopened.is_none() {
            reopened = Some(idx);
        }
    }
    let cur_unresolved = session
        .conflicts
        .get(session.cur)
        .is_some_and(|p| !p.block.resolution.is_resolved());
    if let Some(idx) = reopened {
        session.cur = idx;
    } else if !cur_unresolved {
        session.cur = session
            .conflicts
            .iter()
            .position(|p| !p.block.resolution.is_resolved())
            .unwrap_or(session.cur);
    }
    if unresolved_before > 0 && session.unresolved_count() == 0 {
        messages::info(app, GATE_OPEN_STATUS);
    }
}
