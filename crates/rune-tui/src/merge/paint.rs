//! Merge view background painting (plan WP5.S2/S3): every UNRESOLVED
//! block's marker lines, ours span, and theirs span get their own
//! background, computed as byte intervals derived purely from `Block::
//! start` plus [`super::frame::frame_block`]'s own fixed marker-line
//! lengths. A resolved block contributes no
//! interval at all: its span no longer holds markers, just plain accepted
//! content.
//!
//! Painting itself follows the `code_bg::paint_code_background` precedent
//! (`render/code_bg.rs`), generalized from row-rectangles to byte-interval
//! regions: rather than a fresh byte->row/col walk, it keys directly off
//! `Cell::buf_offset` — the same chokepoint `overlay::highlight_selection`
//! already paints a byte-range background through — so the "cells for
//! display, bytes for spans" rule is never in tension with this pass.

use ratatui::style::Modifier;

use super::state::{Block, Conflict, MergeState};
use crate::document::DocumentId;
use crate::render::{Cell, paint_range};
use crate::theme::Theme;

/// [`super::frame::frame_block`]'s exact framing-line shape: the ours
/// marker line and the separator line between ours and theirs, each
/// including its own trailing `\n`. `frame_block` never varies this, so
/// every interval below is arithmetic on these two constants plus the
/// immutable conflict's own ours/theirs byte lengths — no re-parse of the
/// buffer's current bytes.
const OURS_MARKER_LINE_LEN: usize = "<<<<<<< editor\n".len();
const SEP_MARKER_LINE_LEN: usize = "=======\n".len();

/// Paints `rows` for `state`'s current merge attempt onto `active_doc`'s
/// display cells. A no-op outside `Active`, and a no-op when `active_doc`
/// isn't the merge's own document — merge mode paints only the document it
/// took over, never a different pane's buffer that merely happens to share
/// a render pass.
pub(crate) fn paint(
    rows: &mut [Vec<Cell>],
    state: &MergeState,
    active_doc: DocumentId,
    theme: &Theme,
) {
    let MergeState::Active {
        doc,
        conflicts,
        blocks,
        cur,
        ..
    } = state
    else {
        return;
    };
    if *doc != active_doc {
        return;
    }
    for (k, block) in blocks.iter().enumerate() {
        if block.resolved {
            continue;
        }
        let Some(conflict) = conflicts.get(k) else {
            continue;
        };
        paint_block(rows, block, conflict, k == *cur, theme);
    }
}

/// One unresolved block's five intervals: marker / ours / marker / theirs /
/// marker, in buffer order. The current block's marker lines carry an
/// added `BOLD` on top of the ordinary `merge_marker_bg` — the plan's
/// "distinct cue" so `[`/`]` navigation has visible feedback beyond the
/// status line.
fn paint_block(
    rows: &mut [Vec<Cell>],
    block: &Block,
    conflict: &Conflict,
    is_current: bool,
    theme: &Theme,
) {
    let ours_start = block.start + OURS_MARKER_LINE_LEN;
    let ours_end = ours_start + conflict.ours.len();
    let theirs_start = ours_end + 1 + SEP_MARKER_LINE_LEN;
    let theirs_end = theirs_start + conflict.theirs.len();

    let marker_style = if is_current {
        theme.chrome.merge_marker_bg.add_modifier(Modifier::BOLD)
    } else {
        theme.chrome.merge_marker_bg
    };

    paint_range(rows, block.start..ours_start, marker_style);
    paint_range(rows, ours_start..ours_end, theme.chrome.merge_ours_bg);
    paint_range(rows, ours_end..theirs_start, marker_style);
    paint_range(rows, theirs_start..theirs_end, theme.chrome.merge_theirs_bg);
    paint_range(rows, theirs_end..block.end, marker_style);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::num::NonZeroU64;

    use ratatui::style::Style;

    use super::*;
    use crate::merge::frame::build_marker_buffer;
    use rune_merge::Hunk;

    fn cell(text: &str, buf_offset: i64) -> Cell {
        Cell {
            text: text.to_string(),
            width: 1,
            style: Style::default(),
            buf_offset,
        }
    }

    fn doc_id() -> DocumentId {
        DocumentId(NonZeroU64::MIN)
    }

    /// One row per byte, in order — enough to exercise `paint_range`'s
    /// offset arithmetic without any real wrap/layout machinery.
    fn rows_for(bytes: &str) -> Vec<Vec<Cell>> {
        bytes
            .bytes()
            .enumerate()
            .map(|(i, b)| vec![cell(&(b as char).to_string(), i as i64)])
            .collect()
    }

    fn style_at(rows: &[Vec<Cell>], offset: usize) -> Style {
        rows[offset][0].style
    }

    #[test]
    fn ours_theirs_and_marker_spans_each_carry_their_own_background() {
        let theme = Theme::catppuccin_mocha(false);
        let hunks = vec![Hunk::Conflict {
            ours: b"mine".to_vec(),
            theirs: b"yours".to_vec(),
        }];
        let (buffer, blocks, conflicts) = build_marker_buffer(&hunks).unwrap();
        let mut rows = rows_for(&buffer);
        let id = doc_id();
        let state = MergeState::Active {
            doc: id,
            conflicts,
            blocks,
            cur: 0,
            saved_display_name: None,
        };

        paint(&mut rows, &state, id, &theme);

        let mine_at = buffer.find("mine").unwrap();
        let yours_at = buffer.find("yours").unwrap();
        let marker_at = buffer.find("<<<<<<<").unwrap();
        let sep_at = buffer.find("=======").unwrap();

        assert_eq!(style_at(&rows, mine_at).bg, theme.chrome.merge_ours_bg.bg);
        assert_eq!(
            style_at(&rows, yours_at).bg,
            theme.chrome.merge_theirs_bg.bg
        );
        assert_eq!(
            style_at(&rows, marker_at).bg,
            theme.chrome.merge_marker_bg.bg
        );
        assert_eq!(style_at(&rows, sep_at).bg, theme.chrome.merge_marker_bg.bg);
    }

    #[test]
    fn a_resolved_block_is_painted_with_no_background_at_all() {
        let theme = Theme::catppuccin_mocha(false);
        let hunks = vec![Hunk::Conflict {
            ours: b"mine".to_vec(),
            theirs: b"yours".to_vec(),
        }];
        let (buffer, mut blocks, conflicts) = build_marker_buffer(&hunks).unwrap();
        blocks[0].resolved = true;
        let mut rows = rows_for(&buffer);
        let id = doc_id();
        let state = MergeState::Active {
            doc: id,
            conflicts,
            blocks,
            cur: 0,
            saved_display_name: None,
        };

        paint(&mut rows, &state, id, &theme);

        for row in &rows {
            assert_eq!(
                row[0].style.bg, None,
                "a resolved block must stay unpainted"
            );
        }
    }

    #[test]
    fn the_current_blocks_marker_cue_differs_from_a_non_current_blocks() {
        let theme = Theme::catppuccin_mocha(false);
        let hunks = vec![
            Hunk::Conflict {
                ours: b"a".to_vec(),
                theirs: b"b".to_vec(),
            },
            Hunk::Clean(b"---\n".to_vec()),
            Hunk::Conflict {
                ours: b"c".to_vec(),
                theirs: b"d".to_vec(),
            },
        ];
        let (buffer, blocks, conflicts) = build_marker_buffer(&hunks).unwrap();
        let mut rows = rows_for(&buffer);
        let id = doc_id();
        let state = MergeState::Active {
            doc: id,
            conflicts,
            blocks: blocks.clone(),
            cur: 0,
            saved_display_name: None,
        };

        paint(&mut rows, &state, id, &theme);

        let current_marker = style_at(&rows, blocks[0].start);
        let other_marker = style_at(&rows, blocks[1].start);
        assert_eq!(current_marker.bg, other_marker.bg);
        assert_ne!(
            current_marker.add_modifier, other_marker.add_modifier,
            "the current block's marker must carry a distinct cue"
        );
    }
}
