//! Hunk resolution (plan WP4.S2): `[`/`]` navigation over unresolved
//! blocks and the O/T/B accept path. Every accepted side is ONE ordinary
//! journaled edit through `commit_edit_batch` — undo/redo over merge
//! decisions needs no machinery beyond the journal itself (decision 1).
//! Block spans are the mutable "where is it now" projection over the
//! immutable `Conflict` list; an accept collapses one span and shifts every
//! later span by the byte delta.

use rune_core::buffer::Edit;
use rune_core::cursor::CursorId;

use crate::app::App;
use crate::commands::edit_core::commit_edit_batch;
use crate::commands::nav_scroll;
use crate::document::DocumentId;
use crate::messages;

use super::state::{Block, ConflictBlock, MergeState};

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
        doc, pairs, cur, ..
    } = &mut app.merge
    else {
        return;
    };
    let doc = *doc;
    let blocks: Vec<Block> = pairs.iter().map(|p| p.block.clone()).collect();
    let Some(next) = next_unresolved(&blocks, *cur, dir) else {
        return;
    };
    *cur = next;
    land_on(app, doc, &blocks, next);
}

/// O/T/B (plan WP4.S2). On an edit refusal (`commit_edit_batch` returning
/// `false`) the block stays unresolved and the user is told — never marked
/// resolved against an edit that never applied.
pub(crate) fn accept(app: &mut App, choice: Choice) {
    let MergeState::Active {
        doc, pairs, cur, ..
    } = &app.merge
    else {
        return;
    };
    let (doc, cur) = (*doc, *cur);
    let Some(pair) = pairs.get(cur) else { return };
    let block = pair.block.clone();
    let text = match choice {
        Choice::Ours => pair.conflict.ours.clone(),
        Choice::Theirs => pair.conflict.theirs.clone(),
        Choice::Both => format!("{}\n{}", pair.conflict.ours, pair.conflict.theirs),
    };

    let Some(document) = app.doc(doc) else { return };
    let cursors_before = document.cursors.clone();
    let edit = Edit {
        start: block.range.start,
        end: block.range.end,
        insert: text.clone(),
    };
    if !commit_edit_batch(app, doc, vec![(edit, CursorId::FIRST)], cursors_before) {
        messages::error(
            app,
            "merge: the block could not be applied — left unresolved",
        );
        return;
    }
    let new_len = text.len();

    let MergeState::Active {
        pairs,
        cur: cur_slot,
        ..
    } = &mut app.merge
    else {
        return;
    };
    let old_len = block.range.end - block.range.start;
    if let Some(p) = pairs.get_mut(cur) {
        p.block.range.end = p.block.range.start + new_len;
        p.block.resolved = true;
    }
    if let Some(later) = pairs.get_mut(cur + 1..) {
        for p in later {
            if new_len >= old_len {
                p.block.range.start += new_len - old_len;
                p.block.range.end += new_len - old_len;
            } else {
                p.block.range.start -= old_len - new_len;
                p.block.range.end -= old_len - new_len;
            }
        }
    }
    let blocks: Vec<Block> = pairs.iter().map(|p| p.block.clone()).collect();
    let next = next_unresolved(&blocks, cur, 1);
    if let Some(n) = next {
        *cur_slot = n;
    }
    let pairs_snapshot: Vec<ConflictBlock> = pairs.clone();

    let marker_content = app
        .doc(doc)
        .map(|d| d.buffer.content().to_string())
        .unwrap_or_default();
    super::persist::enqueue_merge_progress(app, doc, &marker_content, &pairs_snapshot);

    let Some(next) = next else {
        // Decision 13: resolving the last hunk leaves merge mode
        // immediately — `exit_in_place` reports "merge complete".
        super::exit_in_place(app);
        return;
    };
    land_on(app, doc, &blocks, next);
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

pub(super) fn block_start(blocks: &[Block], idx: usize) -> usize {
    blocks.get(idx).map_or(0, |b| b.range.start)
}

pub(super) fn scroll_doc(app: &mut App, doc: DocumentId, byte: usize) {
    if let Some(d) = app.doc_mut(doc) {
        nav_scroll::scroll_to_byte_offset(d, byte);
    }
}

fn land_on(app: &mut App, doc: DocumentId, blocks: &[Block], next: usize) {
    let target = block_start(blocks, next);
    let position = position_status(blocks, next);
    scroll_doc(app, doc, target);
    messages::info(app, position);
}
