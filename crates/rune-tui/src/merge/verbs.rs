use rune_core::buffer::Edit;
use rune_core::cursor::{CursorId, CursorSet};
use rune_core::undo::EditKind;

use crate::app::App;
use crate::commands::edit_core::apply_edit_batch_with_cursors;
use crate::document::DocumentId;
use crate::messages;

use super::session::{BlockOrigin, MergeSession, Resolution};
use super::state::MergeState;

pub(crate) fn active_on(app: &App, id: DocumentId) -> bool {
    matches!(&app.merge, MergeState::Active { doc, .. } if *doc == id)
}

fn nav_order(session: &MergeSession) -> Vec<usize> {
    let conflicts = session
        .conflicts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.block.origin == BlockOrigin::Conflict)
        .map(|(i, _)| i);
    let autos = session
        .conflicts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.block.origin == BlockOrigin::AutoApplied)
        .map(|(i, _)| i);
    conflicts.chain(autos).collect()
}

fn position_status(session: &MergeSession, idx: usize) -> String {
    let Some(pair) = session.conflicts.get(idx) else {
        return String::new();
    };
    let peers: Vec<usize> = session
        .conflicts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.block.origin == pair.block.origin)
        .map(|(i, _)| i)
        .collect();
    let ordinal = peers.iter().position(|&i| i == idx).map_or(0, |p| p + 1);
    let total = peers.len();
    match pair.block.origin {
        BlockOrigin::Conflict => {
            format!("conflict {ordinal}/{total} — {}", super::VERB_HINT)
        }
        BlockOrigin::AutoApplied => {
            format!("applied hunk {ordinal}/{total} — ⇧⌘U reverts to yours")
        }
    }
}

fn caret_to(app: &mut App, doc: DocumentId, byte: usize) {
    if let Some(d) = app.doc_mut(doc) {
        let clamped = rune_core::buffer::clamp_to_char_boundary(d.buffer.content(), byte);
        d.cursors = CursorSet::new(clamped);
    }
}

pub(crate) fn nav(app: &mut App, dir: isize) {
    let MergeState::Active { doc, session } = &mut app.merge else {
        return;
    };
    let doc = *doc;
    let order = nav_order(session);
    if order.is_empty() {
        messages::info(app, "no hunks");
        return;
    }
    let pos = order.iter().position(|&i| i == session.cur);
    let len = order.len() as isize;
    let next_pos = match pos {
        Some(p) => (p as isize + dir).rem_euclid(len) as usize,
        None if dir >= 0 => 0,
        None => order.len() - 1,
    };
    let Some(&idx) = order.get(next_pos) else {
        return;
    };
    session.cur = idx;
    let target = session
        .conflicts
        .get(idx)
        .map_or(0, |p| p.block.range.start);
    let status = position_status(session, idx);
    caret_to(app, doc, target);
    messages::info(app, status);
}

fn current(app: &App) -> Option<(DocumentId, usize)> {
    let MergeState::Active { doc, session } = &app.merge else {
        return None;
    };
    session.conflicts.get(session.cur)?;
    Some((*doc, session.cur))
}

fn block_bytes(app: &App, doc: DocumentId, idx: usize) -> Option<String> {
    let MergeState::Active { session, .. } = &app.merge else {
        return None;
    };
    let range = session.conflicts.get(idx)?.block.range.clone();
    app.doc(doc)?
        .buffer
        .content()
        .get(range)
        .map(str::to_string)
}

fn replace_block(app: &mut App, doc: DocumentId, idx: usize, insert: String) -> bool {
    let Some(range) = (match &app.merge {
        MergeState::Active { session, .. } => {
            session.conflicts.get(idx).map(|p| p.block.range.clone())
        }
        _ => None,
    }) else {
        return false;
    };
    let Some(document) = app.doc(doc) else {
        return false;
    };
    let cursors_before = document.cursors.clone();
    let start = range.start;
    let edit = Edit {
        start: range.start,
        end: range.end,
        insert,
    };
    apply_edit_batch_with_cursors(
        app,
        doc,
        vec![(edit, CursorId::FIRST)],
        &cursors_before,
        EditKind::Other,
        move |_, _| vec![CursorSet::new(start).primary()],
    )
}

fn set_resolution(app: &mut App, idx: usize, resolution: Resolution) {
    let MergeState::Active { session, .. } = &mut app.merge else {
        return;
    };
    session.resolve(idx, resolution);
}

fn after_resolution(app: &mut App, doc: DocumentId) {
    let MergeState::Active { session, .. } = &mut app.merge else {
        return;
    };
    let next = session.next_unresolved(1);
    if let Some(n) = next {
        session.cur = n;
    }
    let pairs = session.conflicts.clone();
    let status = next.map(|n| position_status(session, n));
    let target = next.and_then(|n| session.conflicts.get(n).map(|p| p.block.range.start));

    let content = app
        .doc(doc)
        .map(|d| d.buffer.content().to_string())
        .unwrap_or_default();
    super::persist::enqueue_merge_progress(app, doc, &content, &pairs);

    match (target, status) {
        (Some(target), Some(status)) => {
            caret_to(app, doc, target);
            messages::info(app, status);
        }
        _ => super::exit_in_place(app),
    }
}

pub(crate) fn take_theirs(app: &mut App) {
    let Some((doc, idx)) = current(app) else {
        messages::info(app, "no hunk to take");
        return;
    };
    let (theirs, resolution) = match &app.merge {
        MergeState::Active { session, .. } => {
            let Some(pair) = session.conflicts.get(idx) else {
                return;
            };
            (pair.conflict.theirs.clone(), pair.block.resolution)
        }
        _ => return,
    };
    let Some(current_bytes) = block_bytes(app, doc, idx) else {
        messages::error(
            app,
            "merge: the hunk range no longer matches the buffer — left unresolved",
        );
        return;
    };
    if current_bytes == theirs {
        if resolution.is_resolved() {
            messages::info(app, "already disk's version");
        } else {
            set_resolution(app, idx, Resolution::TookTheirs);
            after_resolution(app, doc);
        }
        return;
    }
    if !replace_block(app, doc, idx, theirs) {
        messages::error(
            app,
            "merge: the hunk could not be applied — left unresolved",
        );
        return;
    }
    set_resolution(app, idx, Resolution::TookTheirs);
    after_resolution(app, doc);
}

pub(crate) fn take_ours(app: &mut App) {
    let Some((doc, idx)) = current(app) else {
        messages::info(app, "no hunk to keep");
        return;
    };
    let (ours, origin, resolution) = match &app.merge {
        MergeState::Active { session, .. } => {
            let Some(pair) = session.conflicts.get(idx) else {
                return;
            };
            (
                pair.conflict.ours.clone(),
                pair.block.origin,
                pair.block.resolution,
            )
        }
        _ => return,
    };
    let Some(current_bytes) = block_bytes(app, doc, idx) else {
        messages::error(
            app,
            "merge: the hunk range no longer matches the buffer — left unresolved",
        );
        return;
    };
    if current_bytes == ours {
        match origin {
            BlockOrigin::Conflict => {
                if resolution == Resolution::KeptOurs {
                    set_resolution(app, idx, Resolution::Unresolved);
                    messages::info(app, "conflict reopened");
                    let pairs = match &app.merge {
                        MergeState::Active { session, .. } => session.conflicts.clone(),
                        _ => return,
                    };
                    let content = app
                        .doc(doc)
                        .map(|d| d.buffer.content().to_string())
                        .unwrap_or_default();
                    super::persist::enqueue_merge_progress(app, doc, &content, &pairs);
                } else {
                    set_resolution(app, idx, Resolution::KeptOurs);
                    messages::info(app, "kept yours — ⇧⌘U reopens");
                    after_resolution(app, doc);
                }
            }
            BlockOrigin::AutoApplied => messages::info(app, "already yours"),
        }
        return;
    }
    if !replace_block(app, doc, idx, ours) {
        messages::error(
            app,
            "merge: the hunk could not be applied — left unresolved",
        );
        return;
    }
    set_resolution(app, idx, Resolution::KeptOurs);
    after_resolution(app, doc);
}
