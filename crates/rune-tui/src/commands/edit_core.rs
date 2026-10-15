use rune_core::buffer::{AppliedEdit, Buffer, Edit, SortedEdits};
use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::{Cursor, CursorId, CursorSet};
use rune_core::undo::{EditKind, Step};

use crate::app::App;
use crate::db_enqueue as db;
use crate::document::DocumentId;
use crate::messages;
use crate::navhistory;

/// Sole buffer-mutating primitive: applies a batch, journals exactly one
/// `Step`, and mirrors it to the async replica. Returns whether a batch
/// actually applied rather than `()`, so a caller can tell a real edit
/// apart from every refusal below — e.g. before advancing a merge
/// resolution's CAS baseline over an install that never landed.
pub(crate) fn apply_edit_batch_with_cursors(
    app: &mut App,
    id: DocumentId,
    infos: Vec<(Edit, CursorId)>,
    cursors_before: &CursorSet,
    kind: EditKind,
    cursors_after: impl FnOnce(&[AppliedEdit], &[CursorId]) -> Vec<Cursor>,
) -> bool {
    apply_edit_batch_with_cursors_on(app, id, infos, cursors_before, kind, |_, applied, ids| {
        cursors_after(applied, ids)
    })
}

/// [`apply_edit_batch_with_cursors`] whose cursor hook also sees the buffer
/// as it reads after the edits, for callers that place cursors by line and
/// column rather than by byte offset.
pub(crate) fn apply_edit_batch_with_cursors_on(
    app: &mut App,
    id: DocumentId,
    infos: Vec<(Edit, CursorId)>,
    cursors_before: &CursorSet,
    kind: EditKind,
    cursors_after: impl FnOnce(&Buffer, &[AppliedEdit], &[CursorId]) -> Vec<Cursor>,
) -> bool {
    apply_owned_and_followers(
        app,
        id,
        infos,
        Vec::new(),
        cursors_before,
        kind,
        |new_buf, applied, owners| {
            let ids: Vec<CursorId> = owners.iter().flatten().copied().collect();
            cursors_after(new_buf, applied, &ids)
        },
    )
}

fn apply_owned_and_followers(
    app: &mut App,
    id: DocumentId,
    mut owned: Vec<(Edit, CursorId)>,
    followers: Vec<Edit>,
    cursors_before: &CursorSet,
    kind: EditKind,
    cursors_after: impl FnOnce(&Buffer, &[AppliedEdit], &[Option<CursorId>]) -> Vec<Cursor>,
) -> bool {
    let Some(doc) = app.doc(id) else { return false };
    if doc.is_read_only() {
        if let Some(message) = doc.read_only.refusal_message() {
            messages::warn_if_new(app, message);
        }
        return false;
    }
    owned.retain(|(edit, _)| !is_noop(edit));
    if owned.is_empty() {
        return false;
    }
    let owned = coalesce_touching_edits(owned);
    let followers = unsubsumed_followers(followers, &owned);
    let mut infos: Vec<(Edit, Option<CursorId>)> = owned
        .into_iter()
        .map(|(edit, cid)| (edit, Some(cid)))
        .chain(followers.into_iter().map(|edit| (edit, None)))
        .collect();
    infos.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(a.0.end.cmp(&b.0.end)));
    if let Some(start) = first_overlap_start(&infos) {
        messages::error(
            app,
            format!("edit failed: overlapping edits at byte {start}"),
        );
        return false;
    }
    infos.sort_by(|a, b| b.0.start.cmp(&a.0.start).then(b.0.end.cmp(&a.0.end)));

    let edits: Vec<Edit> = infos.iter().map(|(e, _)| e.clone()).collect();
    let owners: Vec<Option<CursorId>> = infos.iter().map(|(_, owner)| *owner).collect();

    match doc.buffer.apply_edits(&SortedEdits::sort(&edits)) {
        Ok((new_buf, applied)) => {
            let new_cursors =
                outside_crlf_pairs(&new_buf, cursors_after(&new_buf, &applied, &owners));
            let Some(doc) = app.doc_mut(id) else {
                return false;
            };
            doc.buffer = new_buf;
            doc.cursors = CursorSet::new_from(&new_cursors);
            let cursors_after = doc.cursors.all().to_vec();
            let caret = doc.cursors.primary().position.get();
            doc.journal.push(Step {
                edits: applied.clone(),
                cursors_before: cursors_before.all().to_vec(),
                cursors_after: cursors_after.clone(),
                kind,
            });
            doc.ladder_presses = 0;
            doc.ladder_anchor = None;
            db::append_edit(
                app,
                id,
                &applied,
                cursors_before.all(),
                &cursors_after,
                kind,
            );
            crate::merge::ranges::remap_after_edit_batch(app, id, &applied);
            for ae in applied.iter().rev() {
                app.nav_history
                    .shift(id, ae.start, ae.deleted.len(), ae.insert.len());
            }
            navhistory::record_edit(app, id, caret);
            true
        }
        Err(e) => {
            messages::error(app, format!("edit failed: {e}"));
            false
        }
    }
}

fn outside_crlf_pairs(buf: &Buffer, cursors: Vec<Cursor>) -> Vec<Cursor> {
    cursors
        .into_iter()
        .map(|c| Cursor {
            position: BufferOffset(buf.after_crlf_pair(c.position.get())),
            anchor: BufferOffset(buf.after_crlf_pair(c.anchor.get())),
            ..c
        })
        .collect()
}

fn is_noop(edit: &Edit) -> bool {
    edit.start == edit.end && edit.insert.is_empty()
}

fn unsubsumed_followers(followers: Vec<Edit>, owned: &[(Edit, CursorId)]) -> Vec<Edit> {
    let mut deletions: Vec<(usize, usize)> = owned
        .iter()
        .filter(|(edit, _)| edit.start < edit.end && edit.insert.is_empty())
        .map(|(edit, _)| (edit.start, edit.end))
        .collect();
    deletions.sort_unstable();
    followers
        .into_iter()
        .filter(|follower| !is_noop(follower) && !is_deleted_by(&deletions, follower))
        .collect()
}

fn is_deleted_by(sorted_deletions: &[(usize, usize)], follower: &Edit) -> bool {
    if !follower.insert.is_empty() {
        return false;
    }
    let starting_at_or_before =
        sorted_deletions.partition_point(|&(start, _)| start <= follower.start);
    starting_at_or_before
        .checked_sub(1)
        .and_then(|nearest| sorted_deletions.get(nearest))
        .is_some_and(|&(_, end)| follower.end <= end)
}

/// Two per-cursor edits can derive touching or overlapping byte ranges even
/// when the cursors themselves don't (extending a position forward or
/// backward by a rune or a word can land two independently legitimate
/// cursors on adjacent or overlapping ranges) — `CursorSet::merge` never
/// sees these derived ranges, so this is the only place that can catch it
/// before `Buffer::apply_edits` either rejects the batch outright or lets a
/// touching pair through with post-edit starts that silently collapse to
/// the same offset. Byte-identical edits over a non-zero-width range dedupe
/// to one, keeping the lower cursor id; a zero-width insert is exempt from
/// dedup even when two cursors land on the same point, since each is its
/// own cursor's independent insert, not a duplicate. What survives dedup
/// then coalesces only when the earlier edit in a touching pair is a pure
/// deletion — the one remaining shape where two distinct ranges really are
/// the same edit; a pair that still overlaps after both passes is a
/// genuine conflict, not a shape either pass is meant to resolve.
fn coalesce_touching_edits(infos: Vec<(Edit, CursorId)>) -> Vec<(Edit, CursorId)> {
    let deduped = dedupe_identical_edits(infos);
    rune_core::undo::coalesce_touching_deletes(deduped, CursorId::min)
}

fn dedupe_identical_edits(mut infos: Vec<(Edit, CursorId)>) -> Vec<(Edit, CursorId)> {
    infos.sort_by(|a, b| {
        a.0.start
            .cmp(&b.0.start)
            .then(a.0.end.cmp(&b.0.end))
            .then(a.0.insert.cmp(&b.0.insert))
    });
    let mut deduped: Vec<(Edit, CursorId)> = Vec::with_capacity(infos.len());
    for (edit, cid) in infos {
        let replaces_a_range = edit.start != edit.end;
        match deduped.last_mut() {
            Some(last) if replaces_a_range && last.0 == edit => last.1 = last.1.min(cid),
            _ => deduped.push((edit, cid)),
        }
    }
    deduped
}

fn first_overlap_start<T>(infos: &[(Edit, T)]) -> Option<usize> {
    infos.windows(2).find_map(|w| match w {
        [a, b] if a.0.end > b.0.start => Some(b.0.start),
        _ => None,
    })
}

pub(crate) fn commit_edit_batch(
    app: &mut App,
    id: DocumentId,
    infos: Vec<(Edit, CursorId)>,
    cursors_before: &CursorSet,
    kind: EditKind,
) -> bool {
    commit_edits_with_followers(app, id, infos, Vec::new(), cursors_before, kind)
}

pub(crate) fn commit_edits_with_followers(
    app: &mut App,
    id: DocumentId,
    owned: Vec<(Edit, CursorId)>,
    followers: Vec<Edit>,
    cursors_before: &CursorSet,
    kind: EditKind,
) -> bool {
    apply_owned_and_followers(
        app,
        id,
        owned,
        followers,
        cursors_before,
        kind,
        |_, applied, owners| {
            applied
                .iter()
                .zip(owners)
                .filter_map(|(ae, owner)| {
                    owner.map(|cid| Cursor {
                        position: BufferOffset(ae.end),
                        anchor: BufferOffset(ae.end),
                        desired_col: VisualCol(0),
                        id: cid,
                    })
                })
                .collect()
        },
    )
}

#[cfg(test)]
#[path = "edit_core_tests.rs"]
mod tests;
