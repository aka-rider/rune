use rune_syntax::scope::scope_table;
use rune_ts::{HighlightResult, MAX_SPANS};

// Emitting at width 0 is safe: width only reaches the table-layout path
// (which short-circuits at 0, every subtraction saturating) and the
// thematic-break rule (which ignores it) — an overlay never renders either.
pub(crate) fn markdown_fence_spans(text: &str) -> HighlightResult {
    let plain = scope_table().resolve("text");
    let mut blocks = rune_md::parse::parse(text);
    rune_md::reveal_all(&mut blocks);
    let (lines, _) = rune_md::emit::emit(text, &blocks, 0);
    let mut spans: Vec<_> = lines
        .into_iter()
        .flat_map(|line| line.spans.into_iter())
        .filter(|span| Some(span.scope()) != plain)
        .map(|span| (span.range(), span.scope()))
        .collect();
    let truncated = spans.len() > MAX_SPANS;
    spans.truncate(MAX_SPANS);
    HighlightResult { spans, truncated }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn markdown_fence_spans_tags_a_heading_and_skips_plain_text() {
        let text = "# Title\n\nplain text\n";
        let result = markdown_fence_spans(text);
        let spans = result.spans;

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
        assert!(markdown_fence_spans("").spans.is_empty());
    }
}
