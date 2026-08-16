use rune_core::buffer::Edit;
use rune_core::cursor::CursorId;
use rune_core::undo::EditKind;

use crate::app::App;
use crate::commands::edit_core::commit_edit_batch;
use crate::commands::nav_scroll;
use crate::document::DocumentId;
use crate::messages;

use super::session::{Block, ConflictBlock, Resolution};
use super::state::MergeState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    Ours,
    Theirs,
    Both,
}

pub(crate) fn nav(app: &mut App, dir: isize) {
    let MergeState::Active { doc, session } = &mut app.merge else {
        return;
    };
    let doc = *doc;
    let Some(next) = session.next_unresolved(dir) else {
        return;
    };
    session.cur = next;
    let blocks: Vec<Block> = session.conflicts.iter().map(|p| p.block.clone()).collect();
    land_on(app, doc, &blocks, next);
}

pub(crate) fn accept(app: &mut App, choice: Choice) {
    let MergeState::Active { doc, session } = &app.merge else {
        return;
    };
    let (doc, cur) = (*doc, session.cur);
    let Some(pair) = session.conflicts.get(cur) else {
        return;
    };
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
    if !commit_edit_batch(
        app,
        doc,
        vec![(edit, CursorId::FIRST)],
        &cursors_before,
        EditKind::Other,
    ) {
        messages::error(
            app,
            "merge: the block could not be applied — left unresolved",
        );
        return;
    }
    let new_len = text.len();

    let MergeState::Active { session, .. } = &mut app.merge else {
        return;
    };
    let old_len = block.range.end - block.range.start;
    let resolution = match choice {
        Choice::Ours => Resolution::KeptOurs,
        Choice::Theirs => Resolution::TookTheirs,
        Choice::Both => Resolution::HandEdited,
    };
    if let Some(p) = session.conflicts.get_mut(cur) {
        p.block.range.end = p.block.range.start + new_len;
    }
    session.resolve(cur, resolution);
    if let Some(later) = session.conflicts.get_mut(cur + 1..) {
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
    let blocks: Vec<Block> = session.conflicts.iter().map(|p| p.block.clone()).collect();
    let next = session.next_unresolved(1);
    if let Some(n) = next {
        session.cur = n;
    }
    let pairs_snapshot: Vec<ConflictBlock> = session.conflicts.clone();

    let marker_content = app
        .doc(doc)
        .map(|d| d.buffer.content().to_string())
        .unwrap_or_default();
    super::persist::enqueue_merge_progress(app, doc, &marker_content, &pairs_snapshot);

    let Some(next) = next else {
        super::exit_in_place(app);
        return;
    };
    land_on(app, doc, &blocks, next);
}

fn position_status(blocks: &[Block], cur: usize) -> String {
    let unresolved = blocks
        .iter()
        .filter(|b| !b.resolution.is_resolved())
        .count();
    let ordinal = blocks
        .iter()
        .take(cur + 1)
        .filter(|b| !b.resolution.is_resolved())
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
