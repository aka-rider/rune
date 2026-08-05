//! WP0 blocking spike: proves comrak 0.54's AST `sourcepos` is a 1-based,
//! end-inclusive, UTF-8 BYTE-column coordinate system, and that
//!
//!     start = line_starts[l-1] + (c-1)
//!     end   = line_starts[el-1] + ec
//!
//! recovers the exact source byte range for every node — including ones
//! whose first/last character is multi-byte (CJK, ZWJ emoji sequences).
//! This is the byte-offset model the whole rune-md crate is built on
//! (Gotchas: "comrak sourcepos is 1-based, end-inclusive, byte columns").
//! Sourcepos needs no option: `parse_document` always populates
//! `Ast.sourcepos` on every node, block and inline alike.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use comrak::nodes::{AstNode, NodeValue, Sourcepos};
use comrak::{Arena, parse_document};
use rune_md::parse::{line_starts, options, sourcepos_to_range};

/// The WP0-proven conversion formula, byte-exact — now the shared
/// `rune_md::parse::sourcepos_to_range` (Ground rule 3: "make it a shared fn
/// in `src/parse.rs`, don't duplicate").
fn to_range(starts: &[usize], sp: Sourcepos) -> (usize, usize) {
    let r = sourcepos_to_range(starts, sp);
    (r.start, r.end)
}

/// Depth-first search for the first descendant (root included) whose
/// `NodeValue` satisfies `pred`, asserting the extracted `src[start..end]`
/// equals `expected` — the core fidelity assertion this spike exists to make.
fn assert_node_text<'a>(
    src: &str,
    starts: &[usize],
    root: &'a AstNode<'a>,
    pred: impl Fn(&NodeValue) -> bool,
    expected: &str,
) -> &'a AstNode<'a> {
    let node = std::iter::once(root)
        .chain(root.descendants().skip(1))
        .find(|n| pred(&n.data.borrow().value))
        .unwrap_or_else(|| panic!("no node matching predicate found for expected {expected:?}"));
    let sp = node.data.borrow().sourcepos;
    let (start, end) = to_range(starts, sp);
    let actual = src
        .get(start..end)
        .unwrap_or_else(|| panic!("range [{start},{end}) not on a char boundary in {src:?}"));
    assert_eq!(
        actual, expected,
        "sourcepos {sp} -> byte range [{start},{end}) mismatch for {src:?}"
    );
    node
}

#[test]
fn nested_bold_italic_link_delimiters_are_byte_exact() {
    let src = "**[bo*ld*](url)**";
    let starts = line_starts(src);
    let arena = Arena::new();
    let root = parse_document(&arena, src, &options());

    assert_node_text(src, &starts, root, |v| matches!(v, NodeValue::Strong), src);
    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Link(_)),
        "[bo*ld*](url)",
    );
    assert_node_text(src, &starts, root, |v| matches!(v, NodeValue::Emph), "*ld*");
    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Text(t) if t.as_ref() == "bo"),
        "bo",
    );
    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Text(t) if t.as_ref() == "ld"),
        "ld",
    );
}

#[test]
fn wikilink_range_and_url_are_byte_exact() {
    let src = "[[wiki|label]]";
    let starts = line_starts(src);
    let arena = Arena::new();
    let root = parse_document(&arena, src, &options());

    let node = assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::WikiLink(_)),
        src,
    );
    match &node.data.borrow().value {
        NodeValue::WikiLink(w) => assert_eq!(w.url, "wiki"),
        other => panic!("expected WikiLink, got {other:?}"),
    }
    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Text(t) if t.as_ref() == "label"),
        "label",
    );
}

#[test]
fn cjk_surrounding_bold_is_byte_exact() {
    let src = "汉字テスト **粗体** end";
    let starts = line_starts(src);
    let arena = Arena::new();
    let root = parse_document(&arena, src, &options());

    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Text(t) if t.as_ref() == "汉字テスト "),
        "汉字テスト ",
    );
    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Strong),
        "**粗体**",
    );
    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Text(t) if t.as_ref() == "粗体"),
        "粗体",
    );
    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Text(t) if t.as_ref() == " end"),
        " end",
    );
}

#[test]
fn zwj_emoji_family_preceding_bold_is_byte_exact() {
    let src = "a \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466} **b**";
    let starts = line_starts(src);
    let arena = Arena::new();
    let root = parse_document(&arena, src, &options());

    let text_prefix = "a \u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466} ";
    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Text(t) if t.as_ref() == text_prefix),
        text_prefix,
    );
    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Strong),
        "**b**",
    );
    assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Text(t) if t.as_ref() == "b"),
        "b",
    );
}

#[test]
fn tasklist_marker_and_text_are_byte_exact() {
    let src = "- [x] task";
    let starts = line_starts(src);
    let arena = Arena::new();
    let root = parse_document(&arena, src, &options());

    let item = assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::TaskItem(_)),
        src,
    );
    let text = assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Text(t) if t.as_ref() == "task"),
        "task",
    );
    // The marker range is the gap between the item's start and its first
    // child's start (parent/child range-gap derivation) — proves the
    // "## "-style prefix extraction generalizes to "- [x] ".
    let (item_start, _) = to_range(&starts, item.data.borrow().sourcepos);
    let (text_start, _) = to_range(&starts, text.data.borrow().sourcepos);
    assert_eq!(&src[item_start..text_start], "- [x] ");
}

#[test]
fn heading_marker_and_text_are_byte_exact() {
    let src = "## heading";
    let starts = line_starts(src);
    let arena = Arena::new();
    let root = parse_document(&arena, src, &options());

    let heading = assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Heading(_)),
        src,
    );
    match &heading.data.borrow().value {
        NodeValue::Heading(h) => assert_eq!(h.level, 2),
        other => panic!("expected Heading, got {other:?}"),
    }
    let text = assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::Text(t) if t.as_ref() == "heading"),
        "heading",
    );
    let (heading_start, _) = to_range(&starts, heading.data.borrow().sourcepos);
    let (text_start, _) = to_range(&starts, text.data.borrow().sourcepos);
    assert_eq!(&src[heading_start..text_start], "## ");
}

#[test]
fn fenced_code_block_spans_open_and_close_fences() {
    let src = "```rust\nfn f() {}\n```";
    let starts = line_starts(src);
    let arena = Arena::new();
    let root = parse_document(&arena, src, &options());

    let block = assert_node_text(
        src,
        &starts,
        root,
        |v| matches!(v, NodeValue::CodeBlock(_)),
        src,
    );
    match &block.data.borrow().value {
        NodeValue::CodeBlock(cb) => {
            assert!(cb.fenced);
            assert_eq!(cb.info, "rust");
        }
        other => panic!("expected CodeBlock, got {other:?}"),
    }
}
