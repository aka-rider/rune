//! WP7 tests for `Inline::Image` parsing (`![alt](url)` and `![[target]]`)
//! and the standalone-image-line predicate — split out of `parse::tests`
//! to keep that module (and this crate's `parse/mod.rs`) under the
//! 500-line budget.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::parse;
use crate::element::block::Block;
use crate::element::inline::{Inline, standalone_image};
use rune_syntax::element::ByteRange;

fn text_of(content: &str, r: ByteRange) -> &str {
    content.get(r.start..r.end).unwrap()
}

/// WP7: an inline image now parses to a real `ImageM`, not a plain text
/// run — the pre-WP7 test this replaces (`inline_image_is_plain_text_run`)
/// pinned the opposite behaviour.
#[test]
fn inline_image_is_an_image_machine() {
    let content = "![alt](img.png)\n";
    let blocks = parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph");
    };
    assert!(matches!(p.inlines[0], Inline::Image(_)));
    let Inline::Image(m) = &p.inlines[0] else {
        unreachable!()
    };
    assert_eq!(text_of(content, m.range), "![alt](img.png)");
    assert_eq!(text_of(content, m.alt), "alt");
    assert_eq!(text_of(content, m.target), "img.png");
    assert_eq!(m.target_text, "img.png");
    assert!(!m.is_wikilink);
}

#[test]
fn embed_wikilink_image_parses_target_and_range() {
    let content = "![[note.png]]\n";
    let blocks = parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph");
    };
    assert_eq!(p.inlines.len(), 1);
    let Inline::Image(m) = &p.inlines[0] else {
        panic!("expected an image, got {:?}", p.inlines[0]);
    };
    assert_eq!(text_of(content, m.range), "![[note.png]]");
    assert_eq!(text_of(content, m.target), "note.png");
    assert_eq!(m.target_text, "note.png");
    assert!(m.is_wikilink);
    assert!(m.alt.is_empty());
}

#[test]
fn image_between_words_is_not_a_standalone_line() {
    // Truly inline — text on the SAME line as the image — must still be
    // disqualified even under the new per-line qualification.
    let content = "before ![alt](x.png) after\n";
    let blocks = parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph");
    };
    assert!(standalone_image(content, &p.inlines).is_empty());
}

#[test]
fn list_item_image_is_a_standalone_line() {
    let content = "- ![alt](x.png)\n";
    let blocks = parse(content);
    let Block::List(list) = &blocks[0] else {
        panic!("expected list");
    };
    let Block::Paragraph(p) = &list.items[0].children[0] else {
        panic!("expected paragraph");
    };
    let found = standalone_image(content, &p.inlines);
    assert_eq!(found.len(), 1, "expected exactly one standalone image");
    assert_eq!(text_of(content, found[0].target), "x.png");
}

#[test]
fn whitespace_padded_image_line_is_standalone() {
    let content = "  ![alt](x.png)  \n";
    let blocks = parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph");
    };
    assert_eq!(standalone_image(content, &p.inlines).len(), 1);
}

/// WP1: the reported bug — an embed with prose directly above and below,
/// no blank lines, is ONE markdown paragraph. Qualification is per LINE,
/// so the embed's own line still qualifies even though the paragraph as a
/// whole has substantive text on neighbouring lines. Fails against the
/// pre-WP1 paragraph-scoped `standalone_image` (any non-whitespace text
/// anywhere in the paragraph returned nothing).
#[test]
fn embed_line_inside_multiline_paragraph_is_standalone() {
    let content = "prose directly above\n![[image.png]]\nprose directly below\n";
    let blocks = parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph");
    };
    let found = standalone_image(content, &p.inlines);
    assert_eq!(found.len(), 1, "expected exactly one standalone image");
    assert_eq!(found[0].target_text, "image.png");
    assert_eq!(found[0].line, 1);
}

/// A `Revealed` image (caret sitting on it) must still disqualify its own
/// line — that reveal is what lets the caret collapse an image back to its
/// raw source; a placeholder must not paint over it.
#[test]
fn revealed_image_on_its_own_line_is_not_standalone() {
    let content = "![[image.png]]\n";
    let mut blocks = parse(content);
    let Block::Paragraph(p) = &mut blocks[0] else {
        panic!("expected paragraph");
    };
    let Inline::Image(m) = &mut p.inlines[0] else {
        panic!("expected an image");
    };
    m.sm.transition(rune_syntax::element::RevealState::Revealed);
    assert!(standalone_image(content, &p.inlines).is_empty());
}

/// A paragraph with two qualifying embed lines (separated by a prose line
/// in between, still one paragraph with no blank lines) returns both.
#[test]
fn two_qualifying_lines_in_one_paragraph_return_both() {
    let content = "![[a.png]]\nprose in between\n![[b.png]]\n";
    let blocks = parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph");
    };
    let mut found = standalone_image(content, &p.inlines);
    found.sort_by_key(|m| m.line);
    assert_eq!(found.len(), 2, "expected both embed lines");
    assert_eq!(found[0].target_text, "a.png");
    assert_eq!(found[0].line, 0);
    assert_eq!(found[1].target_text, "b.png");
    assert_eq!(found[1].line, 2);
}
