//! Undo/redo resync (plan WP6.S1, port of Go `mergemode/resync.go` — read
//! its header comment first): a journal jump that did not go through the
//! resolver's own keys (`super::keys::intercept`) can move `cur`'s block
//! spans out from under the immutable `Conflict` list — an accepted block
//! shrinks/grows the buffer, and undo/redo replay those edits without the
//! resolver's own bookkeeping. Called after every `commands::edit::undo`/
//! `redo` while merge is `Active` on the document being undone/redone.
//!
//! Slot-ordered AND content-verifying, exactly like the Go original: each
//! conflict `k`'s ORIGINAL `ours`/`theirs` text never changes, so re-scanning
//! the CURRENT buffer for the byte-exact FULL framed block (never a bare
//! `<<<<<<<` anchor) locates block `k` unambiguously even when the document's
//! own prose quotes literal marker lines — a spurious anchor can never
//! satisfy an exact match against `frame_block(ours[k], theirs[k])` unless it
//! happens to carry those exact bytes framed the same way, in which case
//! calling it block `k` is not a misclassification at all.
use crate::app::App;
use crate::document::DocumentId;

use super::frame::frame_block;
use super::state::{Block, MergeState};

/// Byte offset of the first occurrence of `needle` in `content` at or after
/// `from`, or `None` if absent or `from` doesn't land on a char boundary
/// (never panics on an out-of-range/mid-rune offset — §1.3).
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
pub(crate) fn resync(app: &mut App, doc: DocumentId) {
    if app.merge.doc() != Some(doc) {
        return;
    }
    let MergeState::Active {
        doc,
        conflicts,
        blocks: old_blocks,
        cur,
        saved_display_name,
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
            // clamped inside the live buffer (§1.3), rather than
            // misclassify as an open, un-navigable conflict.
            let at = search_from.min(buffer_len);
            new_blocks.push(Block {
                start: at,
                end: at,
                resolved: true,
            });
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
        // Port of Go `TestResync_QuotedMarkers` (`resync_test.go`): `ours`
        // itself quotes a spurious full marker block. A bare `<<<<<<<`
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
