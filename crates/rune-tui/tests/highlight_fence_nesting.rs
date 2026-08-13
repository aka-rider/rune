//! The container-prefix-leak cases, split off `highlight_fence.rs` (500-line budget):
//! a fence nested in a blockquote or a list item must not feed that
//! container's own repeating marker prefix to the parser as source bytes.
//! Kept apart from the pipeline-equivalence cases because these are all
//! about ONE thing — the prefix-free source reconstruction — and each needs
//! its own paired top-level and nested fixture.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

mod highlight_common;

use std::ops::Range;

use highlight_common::{all_spans, app_for, settle_highlight};
use rune_syntax::ScopeId;

/// Schedules and settles the ONE highlight a fresh markdown document with
/// exactly one resolvable fence produces, and returns the spans it would
/// paint. Used by the container-prefix-leak cases below.
fn fence_highlight_spans(content: &str, path: &str) -> Vec<(Range<usize>, ScopeId)> {
    let mut session = app_for(content, path);
    session.app_mut().sync_view();
    settle_highlight(&mut session);
    all_spans(session.app())
}

/// The same settle as [`fence_highlight_spans`], but returns each span's OWN
/// selected text rather than its range — what the exact-bytes assertions
/// below need.
fn settled_span_texts(content: &str, path: &str) -> Vec<String> {
    let mut session = app_for(content, path);
    session.app_mut().sync_view();
    settle_highlight(&mut session);
    let updated = session.app().active_doc().buffer.content().to_string();
    all_spans(session.app())
        .into_iter()
        .filter_map(|(range, _)| updated.get(range).map(str::to_string))
        .collect()
}

/// A fence nested in a blockquote must not feed the blockquote's own
/// repeating `"> "` prefix to the parser as source bytes. YAML is the
/// language this was measured on (14 spans clean vs. 3 once `"> "` starts
/// corrupting indentation-sensitive structure) — a top-level fence and the
/// byte-identical fence nested one blockquote level deep must produce the
/// exact same span count, since after the prefix-free source is
/// reconstructed, tree-sitter sees byte-identical text either way.
#[test]
fn yaml_fence_nested_in_blockquote_produces_the_same_span_count_as_top_level() {
    let yaml_body = "key: value\nnested:\n  child: 1\nlist:\n  - a\n  - b";

    let top_level = format!("```yaml\n{yaml_body}\n```\n");
    let top_spans = fence_highlight_spans(&top_level, "/x/top.md");
    assert!(
        !top_spans.is_empty(),
        "a top-level yaml fence must produce spans at all"
    );

    // Every line, including the fence markers, gets the SAME "> " prefix —
    // stripping it must reconstruct source byte-identical to `top_level`'s
    // own fence content.
    let quoted_body: String = yaml_body
        .lines()
        .map(|line| format!("> {line}\n"))
        .collect();
    let nested = format!("> ```yaml\n{quoted_body}> ```\n");
    let nested_spans = fence_highlight_spans(&nested, "/x/nested.md");

    assert_eq!(
        nested_spans.len(),
        top_spans.len(),
        "a blockquoted yaml fence must highlight identically to the same \
         fence at top level — a span-count mismatch means the blockquote's \
         own \"> \" prefix leaked into the parsed source \
         (top: {top_spans:?}, nested: {nested_spans:?})"
    );
}

/// The list-item variant: the same prefix-leak applies to a list item's own
/// repeating indent, not just a blockquote's `"> "`. Rust's error recovery
/// absorbs a stray `"> "`, so this uses a structured-enough fixture that a
/// shifted or corrupted source would visibly change the span count.
#[test]
fn rust_fence_nested_in_list_item_produces_the_same_span_count_as_top_level() {
    let top_level = "```rust\nfn main() {\n    let a = 1;\n}\n```\n";
    let top_spans = fence_highlight_spans(top_level, "/x/top.md");
    assert!(
        !top_spans.is_empty(),
        "a top-level rust fence must produce spans at all"
    );

    // A "- " marker is 2 bytes wide, so CommonMark requires every
    // continuation line (the fence markers AND its content) indented by
    // exactly 2 spaces — stripping that indent must reconstruct source
    // byte-identical to `top_level`'s own fence content.
    let nested = "- ```rust\n  fn main() {\n      let a = 1;\n  }\n  ```\n";
    let nested_spans = fence_highlight_spans(nested, "/x/nested.md");

    assert_eq!(
        nested_spans.len(),
        top_spans.len(),
        "a rust fence nested in a list item must highlight identically to \
         the same fence at top level (top: {top_spans:?}, nested: {nested_spans:?})"
    );
}

/// Containment alone ("the span is somewhere inside the fence's buffer
/// bytes") is too weak — a span could still straddle the blockquote's own
/// `"> "` marker bytes and merely happen to stay within the fence's overall
/// extent. This asserts on the EXACT bytes each span selects. YAML, not
/// rust, is deliberately chosen: rust's error recovery absorbs a stray
/// `"> "` so completely that no span ever lands on it either way, which
/// would make the assertion true regardless of whether the fix is present.
#[test]
fn nested_fence_spans_never_select_the_blockquote_prefix_bytes() {
    let content = "> ```yaml\n> key: value\n> nested:\n>   child: 1\n> ```\n";
    let mut session = app_for(content, "/x/notes.md");
    session.app_mut().sync_view();
    settle_highlight(&mut session);

    let updated = session.app().active_doc().buffer.content().to_string();
    let spans = all_spans(session.app());
    assert!(
        !spans.is_empty(),
        "the blockquoted yaml fence must still produce spans"
    );

    let sliced: Vec<&str> = spans
        .iter()
        .filter_map(|(range, _)| updated.get(range.clone()))
        .collect();
    for text in &sliced {
        assert!(
            !text.contains("> "),
            "span text {text:?} carries the blockquote's own \"> \" marker \
             bytes — spans selected {sliced:?}"
        );
    }
    // Exact-token assertions: the fence's real tokens must be selected
    // verbatim.
    for token in ["key", "nested", "child"] {
        assert!(
            sliced.contains(&token),
            "no span selects `{token}` exactly; spans selected {sliced:?}"
        );
    }
}

/// The two-line-token case the assertion above misses: every prefix-leak
/// case up to here uses a single-line YAML token, which never exercises
/// `LineMap::to_buffer`'s cross-line path — `to_buffer` used to resolve each
/// endpoint of a mapped range independently and join them into one
/// contiguous buffer range, silently swallowing whatever container prefix
/// sits in the gap between two non-contiguous lines. A Rust
/// block comment opening on the fence's first content line and closing on
/// its second is the load-bearing repro: one highlight token that must
/// split at the line boundary rather than paint straight through the
/// blockquote's own `"> "`.
#[test]
fn nested_fence_spans_never_select_the_blockquote_prefix_bytes_across_a_two_line_token() {
    let content = "> ```rust\n> /* one\n> two */\n> ```\n";
    let sliced = settled_span_texts(content, "/x/notes.md");

    assert!(
        !sliced.is_empty(),
        "the blockquoted rust fence must still produce spans"
    );
    for text in &sliced {
        assert!(
            !text.contains("> "),
            "span text {text:?} carries the blockquote's own \"> \" marker \
             bytes — spans selected {sliced:?}"
        );
    }
    assert!(
        sliced.iter().any(|text| text.contains("one")),
        "the block comment's first content line must still be selected; \
         spans selected {sliced:?}"
    );
    assert!(
        sliced.iter().any(|text| text.contains("two")),
        "the block comment's second content line must still be selected; \
         spans selected {sliced:?}"
    );
}

/// The list-item pairing of the case above. A BARE list item's own
/// continuation indent is not actually a gap byte at all: rune-md only
/// threads a marker-exclusion hint for a blockquote's repeating `"> "`
/// (`ScanHint::Nested`), never for a list item's indent, so a plain list
/// item's content lines stay buffer-CONTIGUOUS — pinned as-is by
/// `rune-md`'s own code-region tests — and `to_buffer`'s old
/// single-range collapse had no gap to leak there in the first place.
/// Nesting a blockquote INSIDE the list item is what actually produces a
/// repeating prefix built from the list's own indent: the blockquote's
/// marker scan starts from the list's un-hinted physical line start, so the
/// list indent and the blockquote's `"> "` fold into one combined gap
/// (`"\n  > "`) — the same defect class, reached through a list item.
#[test]
fn nested_fence_spans_never_select_the_list_items_own_indent_across_a_two_line_token() {
    let content = "- > ```rust\n  > /* one\n  > two */\n  > ```\n";
    let sliced = settled_span_texts(content, "/x/notes.md");

    assert!(
        !sliced.is_empty(),
        "the fence nested in a list item's blockquote must still produce spans"
    );
    for text in &sliced {
        assert!(
            !text.contains("> "),
            "span text {text:?} carries the list item's own indent folded \
             into the blockquote's \"> \" marker — spans selected {sliced:?}"
        );
    }
    assert!(
        sliced.iter().any(|text| text.contains("one")),
        "the block comment's first content line must still be selected; \
         spans selected {sliced:?}"
    );
    assert!(
        sliced.iter().any(|text| text.contains("two")),
        "the block comment's second content line must still be selected; \
         spans selected {sliced:?}"
    );
}

/// A CRLF line's own range includes its trailing `\r` unless
/// `LineMap` trims it, so a fence written with CRLF endings must never let
/// that `\r` reach the parser — checked here at the same seam the two tests
/// above check the container prefix at: the exact bytes a produced span
/// selects.
#[test]
fn a_crlf_fence_never_lets_a_stray_carriage_return_reach_the_parser() {
    let content = "```rust\r\n/* one\r\ntwo */\r\n```\r\n";
    let sliced = settled_span_texts(content, "/x/crlf.md");

    assert!(
        !sliced.is_empty(),
        "the CRLF rust fence must still produce spans"
    );
    for text in &sliced {
        assert!(
            !text.contains('\r'),
            "span text {text:?} carries a stray \\r that reached the parser"
        );
    }
}
