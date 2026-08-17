#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod conceal_common;

use conceal_common::synced;
use rune_md::element::block::Block;
use rune_syntax::element::RevealState;

fn first_inline(content: &str, cursor_offset: usize) -> RevealState {
    let (_buf, doc) = synced(content, cursor_offset, true);
    match &doc.blocks()[0] {
        Block::Paragraph(p) => p.inlines[0].reveal_state(),
        other => panic!("expected a paragraph, got {other:?}"),
    }
}

#[test]
fn emphasis_reveals_at_range_start() {
    assert_eq!(first_inline("**bold**x", 0), RevealState::Revealed);
}

#[test]
fn emphasis_reveals_at_range_end() {
    assert_eq!(first_inline("**bold**x", 8), RevealState::Revealed);
}

#[test]
fn emphasis_stays_rendered_one_byte_past_range_end() {
    assert_eq!(first_inline("**bold**x", 9), RevealState::Rendered);
}

#[test]
fn inline_code_reveals_at_range_start() {
    assert_eq!(first_inline("`code`x", 0), RevealState::Revealed);
}

#[test]
fn inline_code_reveals_at_range_end() {
    assert_eq!(first_inline("`code`x", 6), RevealState::Revealed);
}

#[test]
fn inline_code_stays_rendered_one_byte_past_range_end() {
    assert_eq!(first_inline("`code`x", 7), RevealState::Rendered);
}

#[test]
fn link_reveals_at_range_start() {
    assert_eq!(first_inline("[text](url)x", 0), RevealState::Revealed);
}

#[test]
fn link_reveals_at_range_end() {
    assert_eq!(first_inline("[text](url)x", 11), RevealState::Revealed);
}

#[test]
fn link_stays_rendered_one_byte_past_range_end() {
    assert_eq!(first_inline("[text](url)x", 12), RevealState::Rendered);
}

#[test]
fn wikilink_reveals_at_range_start() {
    assert_eq!(first_inline("[[target]]x", 0), RevealState::Revealed);
}

#[test]
fn wikilink_reveals_at_range_end() {
    assert_eq!(first_inline("[[target]]x", 10), RevealState::Revealed);
}

#[test]
fn wikilink_stays_rendered_one_byte_past_range_end() {
    assert_eq!(first_inline("[[target]]x", 11), RevealState::Rendered);
}

#[test]
fn image_reveals_at_range_start() {
    assert_eq!(first_inline("![alt](url)x", 0), RevealState::Revealed);
}

#[test]
fn image_reveals_at_range_end() {
    assert_eq!(first_inline("![alt](url)x", 11), RevealState::Revealed);
}

#[test]
fn image_stays_rendered_one_byte_past_range_end() {
    assert_eq!(first_inline("![alt](url)x", 12), RevealState::Rendered);
}

#[test]
fn a_shared_edge_byte_reveals_both_the_emphasis_and_the_adjacent_code_span() {
    let content = "**a**`b`x";
    let (_buf, doc) = synced(content, 5, true);
    let Block::Paragraph(p) = &doc.blocks()[0] else {
        panic!("expected a paragraph, got {:?}", doc.blocks()[0]);
    };
    assert_eq!(p.inlines[0].reveal_state(), RevealState::Revealed);
    assert_eq!(p.inlines[1].reveal_state(), RevealState::Revealed);
}
