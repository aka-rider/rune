use std::ops::Range;

use rune_syntax::ScopeId;

use crate::document::Document;

// The results of several regions are concatenated and then re-sorted:
// concatenating sorted lists is not sorted, and the painter's
// outer-then-inner resolution depends on the order. `sort_by` is stable,
// so spans that tie on both keys keep the capture-yield order `rune_ts`
// gave them — the third key of the painter-order contract.
//
// This is also the one clamp covering both channels: `end` is clamped to
// the live content length, and a span whose either endpoint is not a
// `char` boundary (or left empty/inverted by the clamp) is dropped —
// clamping here, rather than on receipt, is what lets the render path
// (which has no `&mut Document`) share it, and means a stored span can be
// stale without ever being unsafe to paint.
pub fn visible_spans(doc: &Document, visible: Range<usize>) -> Vec<(Range<usize>, ScopeId)> {
    let content = doc.buffer.content();
    let len = content.len();
    let mut collected: Vec<(Range<usize>, ScopeId)> = Vec::new();

    for region in &doc.highlight.regions {
        if let Some(tree) = &region.tree
            && let Some(window) = region.map.reconstructed_window(
                crate::linemap::BufOffset(visible.start)..crate::linemap::BufOffset(visible.end),
            )
            && let Some(result) = rune_ts::highlight_range(tree, window.start.0..window.end.0)
        {
            collected.extend(result.spans.into_iter().flat_map(|(range, scope)| {
                region
                    .map
                    .to_buffer(
                        crate::linemap::ReconOffset(range.start)
                            ..crate::linemap::ReconOffset(range.end),
                    )
                    .into_iter()
                    .map(move |piece| (piece.range(), scope))
            }));
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
