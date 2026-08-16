use crate::app::App;
use crate::document::DocumentId;

use super::frame::frame_block;
use super::session::{Block, ConflictBlock, Resolution};
use super::state::MergeState;

fn intersects(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> bool {
    a_start < b_end && b_start < a_end
}

fn index_from(content: &str, needle: &str, from: usize) -> Option<usize> {
    content.get(from..)?.find(needle).map(|i| i + from)
}

fn first_unresolved_from(blocks: &[Block], from: usize) -> Option<usize> {
    let n = blocks.len();
    (0..n)
        .map(|i| (from + i) % n)
        .find(|&i| blocks.get(i).is_some_and(|b| !b.resolution.is_resolved()))
}

pub(crate) fn resync(app: &mut App, doc: DocumentId, affected: Option<&std::ops::Range<usize>>) {
    if app.merge.doc() != Some(doc) {
        return;
    }
    let MergeState::Active { doc, session } = std::mem::take(&mut app.merge) else {
        return;
    };
    let old_pairs = session.conflicts;
    let cur = session.cur;
    let saved_display_name = session.saved_display_name;
    let theirs_obs = session.theirs_obs;

    let content = app
        .doc(doc)
        .map(|d| d.buffer.content().to_string())
        .unwrap_or_default();
    let buffer_len = content.len();

    let mut new_blocks = Vec::with_capacity(old_pairs.len());
    let mut search_from = 0usize;
    for pair in &old_pairs {
        let c = &pair.conflict;
        let framed = frame_block(&c.ours, &c.theirs);
        if let Some(idx) = index_from(&content, &framed, search_from) {
            new_blocks.push(Block {
                range: idx..idx + framed.len(),
                resolution: Resolution::Unresolved,
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
                range: o..o + c.ours.len(),
                resolution: Resolution::KeptOurs,
            });
            search_from = o + c.ours.len();
        } else if let Some(t) = theirs_idx {
            new_blocks.push(Block {
                range: t..t + c.theirs.len(),
                resolution: Resolution::TookTheirs,
            });
            search_from = t + c.theirs.len();
        } else {
            let at = search_from.min(buffer_len);
            new_blocks.push(Block {
                range: at..at,
                resolution: Resolution::HandEdited,
            });
        }
    }

    if let Some(range) = affected {
        for (new, old) in new_blocks.iter_mut().zip(old_pairs.iter()) {
            if old.block.resolution.is_resolved()
                && !intersects(
                    old.block.range.start,
                    old.block.range.end,
                    range.start,
                    range.end,
                )
            {
                new.resolution = old.block.resolution;
            }
        }
    }

    let reopened = old_pairs
        .iter()
        .zip(new_blocks.iter())
        .position(|(old, new)| old.block.resolution.is_resolved() && !new.resolution.is_resolved());
    let new_cur = reopened.or_else(|| {
        if new_blocks
            .get(cur)
            .is_some_and(|b| !b.resolution.is_resolved())
        {
            Some(cur)
        } else {
            first_unresolved_from(&new_blocks, cur.min(new_blocks.len().saturating_sub(1)))
        }
    });

    let new_pairs: Vec<ConflictBlock> = old_pairs
        .into_iter()
        .zip(new_blocks)
        .map(|(old, block)| ConflictBlock {
            conflict: old.conflict,
            block,
        })
        .collect();

    app.merge = MergeState::Active {
        doc,
        session: super::session::MergeSession {
            conflicts: new_pairs,
            cur: new_cur.unwrap_or(0),
            saved_display_name,
            theirs_obs,
        },
    };

    if new_cur.is_none() {
        super::exit_in_place(app);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::merge::session::Conflict;

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
                range: 0..0,
                resolution: Resolution::TookTheirs,
            },
            Block {
                range: 0..0,
                resolution: Resolution::Unresolved,
            },
            Block {
                range: 0..0,
                resolution: Resolution::KeptOurs,
            },
        ];
        assert_eq!(first_unresolved_from(&blocks, 2), Some(1));
    }

    #[test]
    fn quoted_marker_prose_does_not_fool_a_full_framed_search() {
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
