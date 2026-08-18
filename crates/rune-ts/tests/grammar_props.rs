//! Grammar-robustness property test: every one of the 22 grammars must
//! survive arbitrary bytes without panicking, and a returned highlight
//! result must stay in-bounds, land on `char` boundaries, respect the span
//! cap, and stay in painter order. Every language is walked exhaustively
//! (a random sample could silently skip one); the *source text* fed to each
//! is what proptest randomizes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::ops::Range;
use std::time::Duration;

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use rune_syntax::ScopeId;
use rune_syntax::scope::scope_table;
use rune_ts::highlight::KNOWN_UNRESOLVED_CAPTURES;
use rune_ts::lang::LANGUAGES;
use rune_ts::registry::registry;
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
    if let Some(result) = highlight(lang, source, BUDGET) {
        assert_result_shape(lang, source, &result.spans);
    }
}

#[test]
fn empty_source_never_panics() {
    for def in LANGUAGES {
        match highlight(def.name, "", BUDGET) {
            None => {}
            Some(result) => assert!(
                result.spans.is_empty(),
                "{}: expected no spans for empty input, got {:?}",
                def.name,
                result.spans
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

/// Guards the closed scope vocabulary against a future grammar or query
/// bump: every capture name every one of the 22 compiled queries actually
/// uses (bundled by a grammar crate, or hand-authored in this crate's
/// `queries/`, such as `terraform.scm`) must resolve against the same
/// closed [`ScopeTable`] the highlight path resolves against at runtime —
/// unless it is one of [`KNOWN_UNRESOLVED_CAPTURES`], a gap that predates
/// this test. An unresolved capture outside that list means a grammar
/// update introduced a scope name this crate's vocabulary doesn't cover —
/// it must fail this test instead of silently dropping a highlight at
/// runtime.
///
/// [`ScopeTable`]: rune_syntax::ScopeTable
#[test]
fn every_compiled_query_capture_resolves_against_the_closed_scope_table() {
    let scopes = scope_table();
    let reg = registry();
    let mut checked = 0usize;
    for def in LANGUAGES {
        let id = rune_ts::resolve(def.name)
            .unwrap_or_else(|| panic!("{}: not resolvable via rune_ts::resolve", def.name));
        let (_language, query) = reg.get(id).unwrap_or_else(|| {
            panic!(
                "{}: query failed to compile: {:?}",
                def.name,
                reg.failures()
            )
        });
        for name in query.capture_names() {
            checked += 1;
            if scopes.resolve(name).is_none() {
                assert!(
                    KNOWN_UNRESOLVED_CAPTURES.contains(&(def.name, name)),
                    "{}: capture @{name} does not resolve against the closed scope table \
                     and is not in KNOWN_UNRESOLVED_CAPTURES",
                    def.name
                );
            }
        }
    }
    assert!(checked > 0, "no capture names were checked");
}

/// Keeps [`KNOWN_UNRESOLVED_CAPTURES`] from silently drifting: every listed
/// pair must still name a capture the relevant query actually contains, and
/// that capture must still fail to resolve — otherwise the entry is stale
/// (the grammar dropped the capture, or the scope table grew to cover it)
/// and should be deleted rather than left to mask a real regression later.
#[test]
fn known_unresolved_captures_are_still_accurate() {
    let scopes = scope_table();
    let reg = registry();
    for (lang, capture) in KNOWN_UNRESOLVED_CAPTURES {
        let id = rune_ts::resolve(lang).unwrap_or_else(|| panic!("{lang}: unresolvable"));
        let (_language, query) = reg
            .get(id)
            .unwrap_or_else(|| panic!("{lang}: query failed to compile"));
        assert!(
            query.capture_names().contains(capture),
            "{lang}: KNOWN_UNRESOLVED_CAPTURES lists @{capture} but the query no longer uses it"
        );
        assert!(
            scopes.resolve(capture).is_none(),
            "{lang}: KNOWN_UNRESOLVED_CAPTURES lists @{capture} but it now resolves — remove the entry"
        );
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
