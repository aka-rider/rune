//! The tab expansion rests on one claim: the copy comrak actually parses
//! is structure-preserving. CommonMark advances a tab to the next
//! four-column stop wherever spaces define block structure, so writing
//! those spaces out must leave comrak every block decision it would have
//! made on the document itself. These properties mechanise that claim, and
//! the downstream one it buys: every range the pipeline reports still
//! indexes the REAL bytes.
//!
//! Blanking a lone carriage return is the copy's OTHER job and is
//! deliberately NOT structure-preserving — CommonMark ends a line on a
//! bare CR and this crate's buffer does not, and reconciling the two is
//! the entire point. So the baseline here is the document with that one
//! reconciliation already applied; what remains between it and the copy is
//! exactly the tab expansion. That the reconciliation lands is asserted
//! directly instead: comrak's line numbers on the copy must index the real
//! document's lines.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, parse_document};
use proptest::prelude::*;
use rune_core::buffer::Buffer;
use rune_md::emit::emit;
use rune_md::invariant::assert_full_line_coverage;
use rune_md::parse::{line_starts, options, parse, parse_shadow};
use rune_md::reveal_all;
use std::mem::{Discriminant, discriminant};

/// The block decisions under test, in pre-order: WHICH node, and WHICH
/// lines it spans. The node's owned content is deliberately absent — a
/// leading tab that became spaces legitimately changes a code block's
/// literal text — and so are columns, which shifting is the whole point.
fn block_shape<'a>(document: &'a AstNode<'a>) -> Vec<(Discriminant<NodeValue>, usize, usize)> {
    document
        .descendants()
        .map(|node| {
            let ast = node.data.borrow();
            (
                discriminant(&ast.value),
                ast.sourcepos.start.line,
                ast.sourcepos.end.line,
            )
        })
        .collect()
}

fn readable_shape<'a>(document: &'a AstNode<'a>) -> String {
    document
        .descendants()
        .map(|node| {
            let ast = node.data.borrow();
            format!(
                "  {} {}..{}",
                variant_name(&ast.value),
                ast.sourcepos.start.line,
                ast.sourcepos.end.line
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn variant_name(value: &NodeValue) -> String {
    format!("{value:?}")
        .chars()
        .take_while(|c| c.is_alphanumeric())
        .collect()
}

fn with_lone_carriage_returns_blanked(content: &str) -> String {
    let bytes = content.as_bytes();
    content
        .char_indices()
        .map(|(at, c)| {
            if c == '\r' && bytes.get(at + 1) != Some(&b'\n') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// The bytes the copy is built out of: container-prefix material (space,
/// tab, `>`), the block openers a prefix column decides between, a lone
/// carriage return, a multi-byte character, and a byte-order mark.
fn arb_token() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("\t"),
        Just("  "),
        Just(">"),
        Just("> "),
        Just("-"),
        Just("- "),
        Just("1."),
        Just("#"),
        Just("```"),
        Just("|"),
        Just("---"),
        Just("`"),
        Just("\r"),
        Just("a"),
        Just("\u{4f60}"),
        Just("\u{feff}"),
    ]
}

fn arb_line() -> impl Strategy<Value = String> {
    proptest::collection::vec(arb_token(), 0..5).prop_map(|tokens| tokens.concat())
}

fn arb_document() -> impl Strategy<Value = String> {
    proptest::collection::vec(arb_line(), 0..6).prop_map(|lines| lines.join("\n"))
}

fn assert_block_shape_preserved(content: &str) {
    let shadow = parse_shadow(content);
    let baseline = with_lone_carriage_returns_blanked(content);
    let arena = Arena::new();
    let document = parse_document(&arena, &baseline, &options());
    let copy = parse_document(&arena, &shadow, &options());
    assert_eq!(
        block_shape(document),
        block_shape(copy),
        "the copy parses to a different block structure\n\
         document: {:?}\ncopy:     {:?}\nfrom the document:\n{}\nfrom the copy:\n{}",
        baseline,
        shadow,
        readable_shape(document),
        readable_shape(copy)
    );
}

#[test]
fn a_trailing_tab_on_a_lazy_continuation_stays_a_soft_break() {
    assert_block_shape_preserved("1.>\n\t>\t\na");
}

#[test]
fn a_trailing_tab_after_a_table_like_line_stays_a_soft_break() {
    assert_block_shape_preserved("|\t\n\t>\t\na");
}

#[test]
fn a_trailing_tab_before_a_crlf_ending_stays_a_soft_break() {
    assert_block_shape_preserved("1.>\n\t>\t\r\na");
}

#[test]
fn a_trailing_tab_padded_by_spaces_stays_a_soft_break() {
    assert_block_shape_preserved("1.>\n\t> \t \na");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn expanding_the_tabs_keeps_every_block_decision(content in arb_document()) {
        let shadow = parse_shadow(&content);
        let baseline = with_lone_carriage_returns_blanked(&content);
        let arena = Arena::new();
        let document = parse_document(&arena, &baseline, &options());
        let copy = parse_document(&arena, &shadow, &options());

        prop_assert_eq!(
            block_shape(document),
            block_shape(copy),
            "the copy parses to a different block structure\n\
             document: {:?}\ncopy:     {:?}\nfrom the document:\n{}\nfrom the copy:\n{}",
            baseline,
            shadow,
            readable_shape(document),
            readable_shape(copy)
        );

        let line_count = line_starts(&content).len();
        for node in copy.descendants() {
            let sourcepos = node.data.borrow().sourcepos;
            prop_assert!(
                sourcepos.end.line <= line_count,
                "the copy's line {} is not a line of {:?} (it has {})",
                sourcepos.end.line, content, line_count
            );
        }
    }

    #[test]
    fn every_emitted_range_indexes_the_real_document(content in arb_document()) {
        let mut blocks = parse(&content);
        reveal_all(&mut blocks);
        let (lines, snapshot) = emit(&content, &blocks, 80);

        for (line, rendered) in lines.iter().enumerate() {
            for span in &rendered.spans {
                let range = span.range();
                prop_assert!(
                    range.start <= range.end,
                    "line {} span {:?}: inverted range in {:?}", line, span, content
                );
                prop_assert!(
                    range.end <= content.len(),
                    "line {} span {:?}: range past the end of {:?}", line, span, content
                );
                prop_assert!(
                    content.is_char_boundary(range.start) && content.is_char_boundary(range.end),
                    "line {} span {:?}: range splits a character of {:?}", line, span, content
                );
            }
        }

        assert_full_line_coverage(&Buffer::new(&content), &lines, &snapshot);
    }
}
