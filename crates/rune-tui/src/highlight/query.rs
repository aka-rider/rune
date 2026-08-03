//! The single parse -> render seam: turning a document's stored region
//! highlight state into the painter-ordered spans covering one visible byte
//! window.
//!
//! Everything that paints colour reads this and nothing else — the renderer
//! for the window it just laid out, the session fuzzer's snapshot projection
//! for the whole document. Two consumers, one query, so an invariant checked
//! on the fuzzer's output is an invariant checked on what a user sees.

use std::ops::Range;

use rune_syntax::ScopeId;

use crate::document::Document;

/// Every highlight span covering `visible`, in painter order
/// (`start` ASC, `end` DESC), clamped to the live buffer.
///
/// For each region intersecting the window: a tree-backed region has its
/// retained tree queried over the reconstructed window and each result
/// mapped back to buffer offsets; a span-backed region has its stored spans
/// filtered to the window. Both channels of a region are read — in practice
/// a region has exactly one populated, but reading both means neither can be
/// silently dropped by a future change.
///
/// The results of several regions are concatenated and then RE-SORTED:
/// concatenating sorted lists is not sorted, and the painter's
/// outer-then-inner resolution depends on the order. `sort_by` is stable, so
/// spans that tie on both keys keep the capture-yield order `rune_ts` gave
/// them — the third key of the painter-order contract.
///
/// This is also the one clamp (§1.3), covering both channels and both the
/// fence and whole-file paths: `end` is clamped to the live content length,
/// a span whose either endpoint is not a `char` boundary is dropped, and so
/// is one left empty or inverted by the clamp. Clamping here rather than on
/// receipt is what lets the render path — which has no `&mut Document` —
/// share it, and it means a stored span can be stale without ever being
/// unsafe to paint.
pub fn visible_spans(doc: &Document, visible: Range<usize>) -> Vec<(Range<usize>, ScopeId)> {
    let content = doc.buffer.content();
    let len = content.len();
    let mut collected: Vec<(Range<usize>, ScopeId)> = Vec::new();

    for region in &doc.highlight.regions {
        if let Some(tree) = &region.tree
            && let Some(window) = region.map.reconstructed_window(visible.clone())
            && let Some(result) = rune_ts::highlight_range(tree, window)
        {
            collected.extend(
                result
                    .spans
                    .into_iter()
                    .filter_map(|(range, scope)| region.map.to_buffer(range).map(|r| (r, scope))),
            );
        }
        collected.extend(
            region
                .spans
                .iter()
                .filter(|(range, _)| range.start < visible.end && range.end > visible.start)
                .cloned(),
        );
    }

    let mut spans: Vec<(Range<usize>, ScopeId)> = collected
        .into_iter()
        .filter_map(|(range, scope)| {
            let start = range.start;
            let end = range.end.min(len);
            let usable =
                start < end && content.is_char_boundary(start) && content.is_char_boundary(end);
            usable.then_some((start..end, scope))
        })
        .collect();
    spans.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(b.0.end.cmp(&a.0.end)));
    spans
}
