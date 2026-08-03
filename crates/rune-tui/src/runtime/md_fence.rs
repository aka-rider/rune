//! Markdown-fence highlighting via comrak reuse (plan WP6.S3): a
//! ```` ```markdown ````/```` ```md ```` fence has no `rune-ts` grammar
//! ("markdown stays comrak's", `rune_ts::lang`'s own doc comment), so this
//! module reuses the SAME emitter that renders the real document, just
//! forced fully revealed, and re-tags its output as overlay spans. CONSTITUTION
//! §12 forbids the reverse direction (tree-sitter output feeding back into an
//! emitted `SyntaxSpan`); emit-output feeding an overlay is the sanctioned
//! channel this module uses instead.

use std::ops::Range;

use rune_syntax::ScopeId;
use rune_syntax::scope::scope_table;

/// Highlights one fence's reconstructed markdown source (plan WP6.S3):
/// parse -> force every block revealed (`rune_md::reveal_all`, plan
/// WP6.S1) -> emit at width 0. Width only ever reaches the table layout
/// path (which short-circuits before using it once available width is 0,
/// every subtraction there saturating) and the thematic-break rule (which
/// ignores it) — a markdown fence rendered as an overlay never lays out a
/// table or draws a rule, so width 0 is safe here. `decor` output (plan
/// WP2) is irrelevant to this path and dropped along with everything else
/// `emit` returns besides the spans.
///
/// Every span's own `(range(), scope())` becomes one overlay span, in the
/// SAME coordinates as `text` (a fresh parse of the reconstructed fence
/// text starts its own byte numbering at 0, matching what `run_fence_
/// highlight`'s `LineMap::to_buffer` expects to remap). The plain
/// `text` scope is skipped for span economy — an overlay carrying a span
/// for every unstyled byte would cost the same paint result at a higher
/// merge/sort cost downstream.
pub(crate) fn markdown_fence_spans(text: &str) -> Vec<(Range<usize>, ScopeId)> {
    let plain = scope_table().resolve("text");
    let mut blocks = rune_md::parse::parse(text);
    rune_md::reveal_all(&mut blocks);
    let (lines, _) = rune_md::emit::emit(text, &blocks, 0);
    lines
        .into_iter()
        .flat_map(|line| line.spans.into_iter())
        .filter(|span| Some(span.scope()) != plain)
        .map(|span| (span.range(), span.scope()))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn markdown_fence_spans_tags_a_heading_and_skips_plain_text() {
        let text = "# Title\n\nplain text\n";
        let spans = markdown_fence_spans(text);

        let heading_scope = scope_table()
            .resolve("markup.heading.1")
            .expect("known scope");
        assert!(
            spans.iter().any(|(_, scope)| *scope == heading_scope),
            "the heading line must carry the markup.heading.1 scope"
        );

        let plain = scope_table().resolve("text");
        assert!(
            spans.iter().all(|(_, scope)| Some(*scope) != plain),
            "plain text spans must be skipped entirely"
        );
    }

    #[test]
    fn markdown_fence_spans_never_panics_on_an_empty_fence() {
        assert!(markdown_fence_spans("").is_empty());
    }
}
