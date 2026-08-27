#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::*;
use crate::element::block::Block;

/// `clone_kind_tag`'s `NodeValue::Document` arm is never reached through
/// `parse()`: comrak's own AST has exactly one `Document` node, the
/// root `build_blocks` iterates the CHILDREN of, never a node
/// `build_block` itself is called on — so this pins the trait method's
/// own contract directly, the same way `catalogue`'s `heading_name`
/// test above pins a branch no real `parse()` input can reach.
#[test]
fn document_classifies_as_its_own_kind_not_the_generic_fallback() {
    assert!(matches!(
        NodeValue::Document.clone_kind_tag(),
        BlockKind::Document
    ));
}

/// An HTML block degrades to `Verbatim` the same way any unrecognized
/// syntax does, but must keep ITS OWN `VerbatimKind::Html` tag rather
/// than falling through to `clone_kind_tag`'s generic
/// `_ => BlockKind::Other` arm (which produces `VerbatimKind::Unknown`
/// instead) — a real, reachable distinction the existing
/// `table_and_html_block_become_verbatim` test (which only checks
/// `matches!(_, Block::Verbatim(_))`) does not pin.
#[test]
fn html_block_keeps_its_own_verbatim_kind() {
    let content = "<div>\nraw\n</div>\n";
    let blocks = crate::parse::parse(content);
    let Block::Verbatim(v) = &blocks[0] else {
        panic!("expected a verbatim block, got {:?}", blocks[0]);
    };
    assert_eq!(v.kind, VerbatimKind::Html);
}

/// One new blockquote level per line (never more than one NEW container
/// per line) keeps every level comrak actually opens attributed to
/// THIS crate's own `MAX_CONTAINER_DEPTH` cap alone — comrak's OWN
/// `MAX_LIST_DEPTH` (also 100) gates only how many containers a SINGLE
/// line may newly open at once, so building the nesting incrementally
/// like this is what lets a depth of 100+ actually form at all.
fn nested_blockquotes(levels: usize) -> String {
    let mut content = String::new();
    for k in 1..=levels {
        content.push_str(&">".repeat(k));
        content.push_str(" x\n");
    }
    content
}

fn deepest_kind(content: &str) -> Block {
    let blocks = crate::parse::parse(content);
    let mut b = blocks.into_iter().next().expect("at least one block");
    loop {
        match b {
            Block::Blockquote(mut bq) => {
                b = bq.children.pop().expect("blockquote must have a child");
            }
            other => return other,
        }
    }
}

/// A list met at `depth == 99` (inside 99 blockquote levels) is still a
/// real `List`; at `depth == 100` the `BlockKind::List` cap guard is the
/// only thing degrading it to `Verbatim` — a guard hard-wired to `false`
/// would keep nesting real lists forever.
#[test]
fn a_list_at_the_container_depth_cap_degrades_to_verbatim() {
    let mut under = nested_blockquotes(99);
    under.push_str(&">".repeat(99));
    under.push_str(" - x\n");
    assert!(matches!(deepest_kind(&under), Block::List(_)));

    let mut over = nested_blockquotes(100);
    over.push_str(&">".repeat(100));
    over.push_str(" - x\n");
    assert!(matches!(deepest_kind(&over), Block::Verbatim(_)));
}

/// Exactly 100 real blockquote levels (the 100th built at `depth == 99`,
/// still under the cap) still nest normally; a 101st (built at
/// `depth == 100`) is the first to degrade to `Verbatim` — the precise
/// boundary `depth >= MAX_CONTAINER_DEPTH` draws. A guard hard-wired to
/// `false` would never degrade the 101st level either.
#[test]
fn the_hundred_and_first_nested_blockquote_degrades_to_verbatim() {
    assert!(matches!(
        deepest_kind(&nested_blockquotes(100)),
        Block::Paragraph(_)
    ));
    assert!(matches!(
        deepest_kind(&nested_blockquotes(101)),
        Block::Verbatim(_)
    ));
}

/// A list nested 99 blockquote levels deep is still real (`depth == 99`
/// going in, under the cap); building its OWN item's child depth as
/// `depth + 1` rather than `depth` is what then correctly pushes a
/// further-nested container over the cap at exactly `depth == 100` —
/// pinned by checking that container degrades to `Verbatim`, which a
/// `depth * 1` (no-op) miscount would never do.
#[test]
fn a_list_items_own_child_depth_is_one_more_than_the_lists_own() {
    let mut content = nested_blockquotes(98);
    content.push_str(&">".repeat(99));
    content.push_str(" - > x\n");
    let blocks = crate::parse::parse(&content);
    let mut b = blocks.into_iter().next().expect("at least one block");
    let list = loop {
        match b {
            Block::Blockquote(mut bq) => {
                b = bq.children.pop().expect("blockquote must have a child");
            }
            Block::List(list) => break list,
            other => panic!("expected to reach a list, got {other:?}"),
        }
    };
    let item = list.items.into_iter().next().expect("one list item");
    assert_eq!(item.children.len(), 1);
    assert!(
        matches!(item.children[0], Block::Verbatim(_)),
        "expected the item's own nested blockquote to degrade to Verbatim at depth 100, got {:?}",
        item.children[0]
    );
}

/// `ranges_overlap`'s only real caller (`underline_of_setext_heading`)
/// is a defensive guard for a comrak inline/block desync this crate's
/// own `parse()` cannot currently reproduce (see that function's docs
/// and `catalogue`'s own `setext_heading_name_survives_a_degraded_
/// underline` test) — pinned directly against its own half-open-range
/// contract instead.
#[test]
fn ranges_overlap_treats_touching_ranges_as_not_overlapping() {
    assert!(ranges_overlap(ByteRange::new(0, 10), ByteRange::new(5, 15)));
    assert!(!ranges_overlap(ByteRange::new(0, 5), ByteRange::new(5, 10)));
    assert!(!ranges_overlap(ByteRange::new(5, 10), ByteRange::new(0, 5)));
    assert!(!ranges_overlap(
        ByteRange::new(0, 5),
        ByteRange::new(10, 15)
    ));
}
