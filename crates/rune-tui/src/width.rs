//! The ONE chrome-width chokepoint (plan WP8: display
//! width is terminal CELLS via `unicode-width` over grapheme clusters, not
//! a bare `char`/rune count): every chrome element that measures or
//! truncates user-visible text — footer padding and the always-on `Ln,
//! Col` readout, the breadcrumb, the Explorer root-path title, the Tabs
//! pane's `Open` divider — computes width through `display_width` and
//! truncates through `truncate_to_width`/`truncate_tail_to_width` here,
//! rather than each owning its own `chars().count()` or per-`char` sum. A
//! cluster straddling a measurement boundary (an NFD accent, a ZWJ emoji
//! family) can then never be counted one way and rendered another.

use rune_syntax::wrap::grapheme_width;
use unicode_segmentation::UnicodeSegmentation;

/// The leading-ellipsis marker `truncate_tail_to_width` prepends when it
/// cuts the head off — one display cell.
const TAIL_ELLIPSIS: &str = "\u{2026}";

/// `s`'s width in terminal cells, summed over whole grapheme clusters
/// through the shared width chokepoint (`rune_syntax::wrap::
/// grapheme_width`) — the same number the wrap pass and the renderer's
/// cell segmentation agree on.
pub fn display_width(s: &str) -> usize {
    s.graphemes(true).map(grapheme_width).sum()
}

/// The longest prefix of `s` (cut only at grapheme boundaries) that fits in
/// `max` terminal cells.
pub fn truncate_to_width(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for cluster in s.graphemes(true) {
        let w = grapheme_width(cluster);
        if used + w > max {
            break;
        }
        out.push_str(cluster);
        used += w;
    }
    out
}

/// The longest SUFFIX of `s` (cut only at grapheme boundaries) that fits in
/// `max` terminal cells alongside a leading `…`, used when the TAIL of the
/// text (not its head) is what the reader needs — e.g. Explorer's root
/// path, where the directory's own name and its nearest ancestors matter
/// more than the common prefix every row already shares. Returns `s`
/// unchanged when it already fits; returns an empty string for `max == 0`.
pub fn truncate_tail_to_width(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if display_width(s) <= max {
        return s.to_string();
    }
    let ellipsis_w = display_width(TAIL_ELLIPSIS);
    let budget = max.saturating_sub(ellipsis_w);
    let clusters: Vec<&str> = s.graphemes(true).collect();
    let mut used = 0usize;
    let mut start = clusters.len();
    for (i, cluster) in clusters.iter().enumerate().rev() {
        let w = grapheme_width(cluster);
        if used + w > budget {
            break;
        }
        used += w;
        start = i;
    }
    let tail: String = clusters.get(start..).unwrap_or_default().concat();
    format!("{TAIL_ELLIPSIS}{tail}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn display_width_counts_an_nfd_cluster_as_one_cell() {
        assert_eq!(display_width("cafe\u{0301}"), 4);
    }

    #[test]
    fn display_width_counts_cjk_as_two_cells_each() {
        assert_eq!(display_width("\u{4e2d}\u{6587}"), 4);
    }

    #[test]
    fn truncate_to_width_cuts_only_at_grapheme_boundaries() {
        let s = "\u{4e2d}\u{6587}ab";
        // "中" (2 cells) fits a budget of 3, but appending "文" (2 more
        // cells) would overrun it, so the walk stops there — the
        // trailing ASCII is never reached, matching a real terminal's
        // left-to-right fit check rather than skipping the oversized
        // cluster to keep filling.
        assert_eq!(truncate_to_width(s, 3), "\u{4e2d}");
        assert_eq!(truncate_to_width(s, 2), "\u{4e2d}");
        assert_eq!(truncate_to_width(s, 4), "\u{4e2d}\u{6587}");
    }

    #[test]
    fn truncate_tail_to_width_keeps_the_tail_behind_an_ellipsis() {
        let s = "/very/deeply/nested/project/src/components";
        let truncated = truncate_tail_to_width(s, 20);
        assert_eq!(display_width(&truncated), 20);
        assert!(truncated.starts_with('\u{2026}'));
        assert!(truncated.ends_with("components"));
    }

    #[test]
    fn truncate_tail_to_width_is_a_no_op_when_it_already_fits() {
        assert_eq!(truncate_tail_to_width("/short", 20), "/short");
    }

    #[test]
    fn truncate_tail_to_width_respects_wide_clusters_in_the_budget() {
        // A CJK-heavy tail must not overrun the cell budget just because
        // its CHAR count is smaller than its CELL width.
        let s = "prefix/\u{4e2d}\u{6587}\u{4e2d}\u{6587}";
        let truncated = truncate_tail_to_width(s, 6);
        assert!(display_width(&truncated) <= 6);
    }
}
