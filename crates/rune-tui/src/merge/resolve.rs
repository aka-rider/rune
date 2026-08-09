//! Hunk resolution (plan WP4.S2): `[`/`]` navigation over unresolved
//! blocks and the O/T/B accept path. Every accepted side is ONE ordinary
//! journaled edit through `commit_edit_batch` — undo/redo over merge
//! decisions needs no machinery beyond the journal itself (decision 1).
//! Block spans are the mutable "where is it now" projection over the
//! immutable `Conflict` list; an accept collapses one span and shifts every
//! later span by the byte delta.

use rune_core::buffer::Edit;

use crate::app::App;
use crate::commands::edit_core::commit_edit_batch;
use crate::commands::nav_scroll;
use crate::document::DocumentId;
use crate::messages;

use super::state::{Block, MergeState};

/// Which side an accept keeps. `Both` keeps the two bodies, ours first,
/// with no marker lines — an ordinary journaled edit exactly like the
/// single-sided accepts. "Decide later" remains available by exiting
/// merge mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    Ours,
    Theirs,
    Both,
}

/// `[`/`]` (plan WP4.S1): move `cur` to the nearest unresolved block in
/// `dir`, wrapping around and skipping resolved blocks. With only one
/// unresolved block left the wrap lands back on it — navigation never
/// strands `cur` on a resolved block.
pub(crate) fn nav(app: &mut App, dir: isize) {
    let MergeState::Active {
        doc, blocks, cur, ..
    } = &mut app.merge
    else {
        return;
    };
    let doc = *doc;
    let Some(next) = next_unresolved(blocks, *cur, dir) else {
        return;
    };
    *cur = next;
    let target = blocks.get(next).map(|b| b.start).unwrap_or(0);
    let position = position_status(blocks, next);
    scroll_doc(app, doc, target);
    messages::info(app, position);
}

/// O/T/B (plan WP4.S2). On an edit refusal (`commit_edit_batch` returning
/// `false`) the block stays unresolved and the user is told — never marked
/// resolved against an edit that never applied.
pub(crate) fn accept(app: &mut App, choice: Choice) {
    let MergeState::Active {
        doc,
        conflicts,
        blocks,
        cur,
        ..
    } = &app.merge
    else {
        return;
    };
    let (doc, cur) = (*doc, *cur);
    let Some(block) = blocks.get(cur).copied() else {
        return;
    };
    let replacement = conflicts.get(cur).map(|c| match choice {
        Choice::Ours => c.ours.clone(),
        Choice::Theirs => c.theirs.clone(),
        Choice::Both => format!("{}\n{}", c.ours, c.theirs),
    });
    let Some(text) = replacement else { return };

    let Some(document) = app.doc(doc) else { return };
    let cursors_before = document.cursors.clone();
    let edit = Edit {
        start: block.start,
        end: block.end,
        insert: text.clone(),
    };
    if !commit_edit_batch(app, doc, vec![(edit, 0)], cursors_before) {
        messages::error(
            app,
            "merge: the block could not be applied — left unresolved",
        );
        return;
    }
    let new_len = text.len();

    let MergeState::Active {
        blocks,
        conflicts,
        cur: cur_slot,
        ..
    } = &mut app.merge
    else {
        return;
    };
    let old_len = block.end - block.start;
    if let Some(b) = blocks.get_mut(cur) {
        b.end = b.start + new_len;
        b.resolved = true;
    }
    if let Some(later) = blocks.get_mut(cur + 1..) {
        for b in later {
            if new_len >= old_len {
                b.start += new_len - old_len;
                b.end += new_len - old_len;
            } else {
                b.start -= old_len - new_len;
                b.end -= old_len - new_len;
            }
        }
    }
    let next = next_unresolved(blocks, cur, 1);
    if let Some(n) = next {
        *cur_slot = n;
    }
    let blocks_snapshot = blocks.clone();
    let conflicts_snapshot = conflicts.clone();

    let marker_content = app
        .doc(doc)
        .map(|d| d.buffer.content().to_string())
        .unwrap_or_default();
    super::persist::enqueue_merge_progress(
        app,
        doc,
        &marker_content,
        &blocks_snapshot,
        &conflicts_snapshot,
    );

    let Some(next) = next else {
        // Decision 13: resolving the last hunk leaves merge mode
        // immediately — `exit_in_place` reports "merge complete".
        super::exit_in_place(app);
        return;
    };
    let target = blocks_snapshot.get(next).map(|b| b.start).unwrap_or(0);
    let position = position_status(&blocks_snapshot, next);
    scroll_doc(app, doc, target);
    messages::info(app, position);
}

/// The nearest unresolved index from `from` in `dir`, wrapping; `None` only
/// when every block is resolved. Scans `from` itself last, so with a single
/// unresolved block left both directions land back on it.
fn next_unresolved(blocks: &[Block], from: usize, dir: isize) -> Option<usize> {
    let n = blocks.len();
    if n == 0 {
        return None;
    }
    (1..=n)
        .map(|i| {
            let signed = from as isize + dir * i as isize;
            signed.rem_euclid(n as isize) as usize
        })
        .find(|idx| blocks.get(*idx).is_some_and(|b| !b.resolved))
}

fn position_status(blocks: &[Block], cur: usize) -> String {
    let unresolved = blocks.iter().filter(|b| !b.resolved).count();
    let ordinal = blocks
        .iter()
        .take(cur + 1)
        .filter(|b| !b.resolved)
        .count()
        .max(1);
    format!("conflict {ordinal}/{unresolved} — [O]urs [T]heirs [B]oth")
}

fn scroll_doc(app: &mut App, doc: DocumentId, byte: usize) {
    if let Some(d) = app.doc_mut(doc) {
        nav_scroll::scroll_to_byte_offset(d, byte);
    }
}
