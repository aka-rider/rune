//! Undo/redo resync (plan WP6.S1): a journal jump that did not go through
//! the resolver's own keys (`super::keys::intercept`) can move `cur`'s block
//! spans out from under the immutable `Conflict` list — an accepted block
//! shrinks/grows the buffer, and undo/redo replay those edits without the
//! resolver's own bookkeeping. Called after every `commands::edit::undo`/
//! `redo` while merge is `Active` on the document being undone/redone.
//!
//! Slot-ordered AND content-verifying: each
//! conflict `k`'s ORIGINAL `ours`/`theirs` text never changes, so re-scanning
//! the CURRENT buffer for the byte-exact FULL framed block (never a bare
//! `<<<<<<<` anchor) locates block `k` unambiguously even when the document's
//! own prose quotes literal marker lines — a spurious anchor can never
//! satisfy an exact match against `frame_block(ours[k], theirs[k])` unless it
//! happens to carry those exact bytes framed the same way, in which case
//! calling it block `k` is not a misclassification at all.
//!
//! Review fix F1: a scan alone cannot tell a `[B]`-resolved block (its
//! framed bytes deliberately left in place, decision 5) apart from an
//! undecided one — both are byte-identical in the buffer. Re-deriving EVERY
//! block's resolved-ness from the scan would therefore reopen a `B`'d block
//! on any undo/redo anywhere else in the document. `affected` (the byte
//! range the journal jump itself touched, in the same PRE-jump coordinate
//! space `old_blocks`' spans already live in) scopes the reclassification:
//! a block whose OLD state was `resolved: true` and whose OLD span does not
//! intersect `affected` keeps that `resolved: true` verbatim — only its
//! span is re-derived by the scan below (still needed, since earlier
//! blocks resolving/reopening shifts everything after them). A block that
//! WAS unresolved, or whose old span DOES intersect `affected`, takes
//! whatever resolved-ness the scan finds, exactly as before.
use crate::app::App;
use crate::document::DocumentId;

use super::frame::frame_block;
use super::state::{Block, MergeState};

/// Whether `[a_start, a_end)` and `[b_start, b_end)` share at least one
/// byte — a zero-width span (an already-collapsed `B` block with nothing
/// left in the buffer) never intersects anything, matching a journal edit
/// that never touched it.
fn intersects(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> bool {
    a_start < b_end && b_start < a_end
}

/// Byte offset of the first occurrence of `needle` in `content` at or after
/// `from`, or `None` if absent or `from` doesn't land on a char boundary
/// (never panics on an out-of-range/mid-rune offset).
fn index_from(content: &str, needle: &str, from: usize) -> Option<usize> {
    content.get(from..)?.find(needle).map(|i| i + from)
}

/// The nearest unresolved index at-or-after `from`, wrapping — this port's
/// own deterministic tie-break (plan WP6.S1 asks for one): re-deriving `cur`
/// from scratch after a journal jump keeps the resolver's cursor as close as
/// possible to where the user just was, rather than always snapping back to
/// the first conflict in the document.
fn first_unresolved_from(blocks: &[Block], from: usize) -> Option<usize> {
    let n = blocks.len();
    (0..n)
        .map(|i| (from + i) % n)
        .find(|&i| blocks.get(i).is_some_and(|b| !b.resolved))
}

/// Plan WP6.S1: re-derives `blocks`/`cur` from the LIVE buffer against the
/// immutable `conflicts` list an `Active` merge already carries. A no-op
/// unless merge is `Active` ON `doc` specifically. Ends the merge (via
/// `exit_in_place`, decision 13) when zero blocks come back unresolved —
/// including the case where undo unwound past the working-form install
/// entirely, so every conflict resolves to a plain ours/theirs match or
/// degrades to resolved-without-advance below: this never leaves an `Active`
/// state pointing at spans the buffer no longer has.
///
/// `affected` is the byte range (PRE-jump coordinates, same space as the
/// stored `Block` spans) the undo/redo call actually touched — `None` keeps
/// every block's resolved-ness scan-derived, for callers with no such range
/// to offer. See the module doc for why this scoping exists (review F1).
pub(crate) fn resync(app: &mut App, doc: DocumentId, affected: Option<std::ops::Range<usize>>) {
    if app.merge.doc() != Some(doc) {
        return;
    }
    let MergeState::Active {
        doc,
        conflicts,
        blocks: old_blocks,
        cur,
        saved_display_name,
        theirs_obs,
    } = std::mem::take(&mut app.merge)
    else {
        return;
    };

    let content = app
        .doc(doc)
        .map(|d| d.buffer.content().to_string())
        .unwrap_or_default();
    let buffer_len = content.len();

    let mut new_blocks = Vec::with_capacity(conflicts.len());
    let mut search_from = 0usize;
    for c in &conflicts {
        let framed = frame_block(&c.ours, &c.theirs);
        if let Some(idx) = index_from(&content, &framed, search_from) {
            new_blocks.push(Block {
                start: idx,
                end: idx + framed.len(),
                resolved: false,
            });
            search_from = idx + framed.len();
            continue;
        }

        let ours_idx = index_from(&content, &c.ours, search_from);
        let theirs_idx = index_from(&content, &c.theirs, search_from);
        let ours_first = match (ours_idx, theirs_idx) {
            (Some(o), Some(t)) => o <= t,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if ours_first && let Some(o) = ours_idx {
            new_blocks.push(Block {
                start: o,
                end: o + c.ours.len(),
                resolved: true,
            });
            search_from = o + c.ours.len();
        } else if let Some(t) = theirs_idx {
            new_blocks.push(Block {
                start: t,
                end: t + c.theirs.len(),
                resolved: true,
            });
            search_from = t + c.theirs.len();
        } else {
            // Neither the framed block nor either resolved form is
            // present — an inconsistent buffer the merge invariants
            // should prevent (or undo unwound past the install
            // entirely). Degrade safely: mark resolved with no advance,
            // clamped inside the live buffer, rather than
            // misclassify as an open, un-navigable conflict.
            let at = search_from.min(buffer_len);
            new_blocks.push(Block {
                start: at,
                end: at,
                resolved: true,
            });
        }
    }

    // Review fix F1: force back to `resolved: true` any block the scan
    // above may have reopened by byte-pattern coincidence, PROVIDED it
    // wasn't in the range this journal jump actually touched — a `B`'d
    // block's framed bytes are indistinguishable from an undecided one by
    // content alone, so only the affected range (or an absent one, meaning
    // "trust the scan everywhere") may downgrade a previously-resolved
    // block back to open.
    if let Some(range) = &affected {
        for (new, old) in new_blocks.iter_mut().zip(old_blocks.iter()) {
            if old.resolved && !intersects(old.start, old.end, range.start, range.end) {
                new.resolved = true;
            }
        }
    }

    // A single undo/redo flips at most one conflict's resolved-ness (each
    // accept is exactly one journal step) — when one DID just get reopened
    // (resolved -> unresolved), that is unambiguously the hunk the user just
    // undid, so `cur` lands there regardless of where it was before. Only
    // when nothing reopened (redo re-resolving something, or a jump that
    // landed clean of any transition at all) does `cur` fall back to "stay
    // put if still valid, else nearest unresolved".
    let reopened = old_blocks
        .iter()
        .zip(new_blocks.iter())
        .position(|(old, new)| old.resolved && !new.resolved);
    let new_cur = reopened.or_else(|| {
        if new_blocks.get(cur).is_some_and(|b| !b.resolved) {
            Some(cur)
        } else {
            first_unresolved_from(&new_blocks, cur.min(new_blocks.len().saturating_sub(1)))
        }
    });

    app.merge = MergeState::Active {
        doc,
        conflicts,
        blocks: new_blocks,
        cur: new_cur.unwrap_or(0),
        saved_display_name,
        theirs_obs,
    };

    if new_cur.is_none() {
        super::exit_in_place(app);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::merge::state::Conflict;

    fn conflict(ours: &str, theirs: &str) -> Conflict {
        Conflict {
            ours: ours.to_string(),
            theirs: theirs.to_string(),
        }
    }

    #[test]
    fn index_from_never_panics_on_a_mid_rune_offset() {
        let content = "héllo";
        assert_eq!(index_from(content, "llo", 2), None);
    }

    #[test]
    fn first_unresolved_from_wraps() {
        let blocks = vec![
            Block {
                start: 0,
                end: 0,
                resolved: true,
            },
            Block {
                start: 0,
                end: 0,
                resolved: false,
            },
            Block {
                start: 0,
                end: 0,
                resolved: true,
            },
        ];
        assert_eq!(first_unresolved_from(&blocks, 2), Some(1));
    }

    #[test]
    fn quoted_marker_prose_does_not_fool_a_full_framed_search() {
        // `ours` itself quotes a spurious full marker block. A bare `<<<<<<<`
        // anchor scan would misfire on it; the byte-exact framed-block
        // search must not.
        let spurious_ours =
            "a quoted example:\n<<<<<<< fake\nfake ours\n=======\nfake theirs\n>>>>>>> fake\nend\n";
        let real_theirs = "theirs replacement\n";
        let c = conflict(spurious_ours, real_theirs);
        let framed = frame_block(spurious_ours, real_theirs);
        let content = format!("intro\n{framed}outro\n");

        let idx = index_from(&content, &frame_block(&c.ours, &c.theirs), 0);
        assert_eq!(idx, Some("intro\n".len()));
    }
}
