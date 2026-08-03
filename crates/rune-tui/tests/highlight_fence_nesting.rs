//! The container-prefix-leak cases, split off `highlight_fence.rs` (§1.6):
//! a fence nested in a blockquote or a list item must not feed that
//! container's own repeating marker prefix to the parser as source bytes.
//! Kept apart from the pipeline-equivalence cases because these are all
//! about ONE thing — the prefix-free source reconstruction — and each needs
//! its own paired top-level and nested fixture.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod highlight_common;

use std::ops::Range;

use highlight_common::{all_spans, app_for, type_one_char_at_end};
use rune_syntax::ScopeId;
use rune_tui::app::{self, App};
use rune_tui::runtime::{Effects, Msg};

/// Runs the document's pending highlight to completion through the real
/// message path: schedule (by typing one character), run the `Cmd` inline,
/// deliver its reply.
fn settle_highlight(app: &mut App) {
    let mut effects = Effects::default();
    type_one_char_at_end(app, &mut effects);
    assert_eq!(
        effects.cmds.len(),
        1,
        "expected exactly one scheduled highlight cmd"
    );
    let msg = effects
        .cmds
        .remove(0)
        .run()
        .expect("a highlight cmd always replies with Some(Msg::Highlighted)");
    let Msg::Highlighted { .. } = &msg else {
        panic!("expected a Msg::Highlighted reply, got {msg:?}");
    };
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
}

/// Schedules and settles the ONE highlight a fresh markdown document with
/// exactly one resolvable fence produces, and returns the spans it would
/// paint. Used by the container-prefix-leak cases below.
fn fence_highlight_spans(content: &str, path: &str) -> Vec<(Range<usize>, ScopeId)> {
    let mut app = app_for(content, path);
    app.sync_view();
    settle_highlight(&mut app);
    all_spans(&app)
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
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();
    settle_highlight(&mut app);

    let updated = app.active_doc().buffer.content().to_string();
    let spans = all_spans(&app);
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
