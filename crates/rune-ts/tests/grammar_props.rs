//! Grammar-robustness property test: every one of the 22 grammars must
//! survive arbitrary bytes without panicking, and a returned highlight
//! result must stay in-bounds, land on `char` boundaries, respect the span
//! cap, and stay in painter order. Every language is walked exhaustively
//! (a random sample could silently skip one); the *source text* fed to each
//! is what proptest randomizes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::ops::Range;
use std::time::Duration;

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use rune_syntax::ScopeId;
use rune_ts::lang::LANGUAGES;
use rune_ts::{MAX_SPANS, highlight};

/// Generous enough that a real failure means the grammar hung, not that the
/// budget was too tight for a proptest-sized input.
const BUDGET: Duration = Duration::from_secs(5);

/// One fragment of an arbitrary document: ASCII source-like text, CJK,
/// emoji, a CRLF pair, a BOM, or a non-newline control character — the
/// input shapes a real (or hostile) document can hand a grammar.
fn arb_fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-zA-Z0-9_ (){}\\[\\];:.,+=<>!\"'/*-]{0,16}",
        1 => Just("你好，世界".to_string()),
        1 => Just("こんにちは".to_string()),
        1 => Just("😀🚀🎉🧵".to_string()),
        1 => Just("\r\n".to_string()),
        1 => Just("\u{FEFF}".to_string()),
        1 => (0u8..0x20u8).prop_filter_map("skip newline", |b| {
            let ch = char::from(b);
            (ch != '\n').then(|| ch.to_string())
        }),
    ]
}

fn arb_source() -> impl Strategy<Value = String> {
    proptest::collection::vec(arb_fragment(), 0..16).prop_map(|fragments| fragments.concat())
}

/// The shape invariants a `Some` result must satisfy, whatever grammar or
/// source produced it.
fn assert_result_shape(lang: &str, source: &str, spans: &[(Range<usize>, ScopeId)]) {
    assert!(
        spans.len() <= MAX_SPANS,
        "{lang}: span cap exceeded ({} spans)",
        spans.len()
    );
    let mut prev: Option<&(Range<usize>, ScopeId)> = None;
    for span in spans {
        assert!(
            span.0.start < span.0.end,
            "{lang}: empty or inverted span {:?}",
            span.0
        );
        assert!(
            span.0.end <= source.len(),
            "{lang}: span past end of source {:?} (len {})",
            span.0,
            source.len()
        );
        assert!(
            source.is_char_boundary(span.0.start),
            "{lang}: span start not on a char boundary {:?}",
            span.0
        );
        assert!(
            source.is_char_boundary(span.0.end),
            "{lang}: span end not on a char boundary {:?}",
            span.0
        );
        if let Some(p) = prev {
            let painter_ordered =
                p.0.start < span.0.start || (p.0.start == span.0.start && p.0.end >= span.0.end);
            assert!(
                painter_ordered,
                "{lang}: spans out of painter order: {:?} before {:?}",
                p.0, span.0
            );
        }
        prev = Some(span);
    }
}

/// Runs the property for one language over one generated source: the parse
/// must complete (no panic, no abort — this function returning at all is
/// half the assertion), and a `Some` result must satisfy the shape
/// invariants above.
fn check_language(lang: &'static str, source: &str) {
    if let Some(spans) = highlight(lang, source, BUDGET) {
        assert_result_shape(lang, source, &spans);
    }
}

#[test]
fn empty_source_never_panics() {
    for def in LANGUAGES {
        match highlight(def.name, "", BUDGET) {
            None => {}
            Some(spans) => assert!(
                spans.is_empty(),
                "{}: expected no spans for empty input, got {spans:?}",
                def.name
            ),
        }
    }
}

/// The non-ignored member of this pair: a handful of generated sources per
/// language, exercised on every `cargo test -p rune-ts` run.
#[test]
fn every_grammar_survives_a_handful_of_cases() {
    let mut runner = TestRunner::default();
    let strategy = arb_source();
    for def in LANGUAGES {
        for _ in 0..8 {
            let tree = strategy
                .new_tree(&mut runner)
                .expect("source strategy generation");
            check_language(def.name, &tree.current());
        }
    }
}

/// The `#[ignore]`d, heavier member: `make test-grammars` runs this with
/// many more cases per language.
#[test]
#[ignore]
fn every_grammar_survives_many_cases() {
    let mut runner = TestRunner::default();
    let strategy = arb_source();
    for def in LANGUAGES {
        for _ in 0..500 {
            let tree = strategy
                .new_tree(&mut runner)
                .expect("source strategy generation");
            check_language(def.name, &tree.current());
        }
    }
}
