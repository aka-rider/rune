//! Coverage for the compiling half — `registry()` and `highlight()`. The
//! compile-free `lang::resolve` paths have their own suite.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use rune_syntax::{LangId, scope::scope_table};
use rune_ts::{highlight, registry};

#[test]
fn every_language_loads_and_its_query_compiles() {
    let reg = registry();
    // First, trigger compilation of all languages by requesting each one.
    let names: Vec<_> = reg.names().collect();
    for id in LangId::all() {
        let _ = reg.get(id);
    }
    // Now assert that all languages compiled successfully.
    assert!(
        reg.failures().is_empty(),
        "language(s) failed to load or compile: {:?}",
        reg.failures()
    );
    assert_eq!(names.len(), 25);
}

#[test]
fn registry_is_a_stable_shared_instance() {
    let first = registry();
    let rust = LangId::from_name("rust").expect("rust is a known language");
    assert!(
        first.get(rust).is_some(),
        "rust must be resolvable via the shared registry()"
    );
    let second = registry();
    assert!(
        std::ptr::eq(first, second),
        "registry() must hand back the same cached instance on every call, not a fresh one"
    );
    assert_eq!(
        second.names().count(),
        25,
        "the shared registry must list all 25 languages"
    );
}

#[test]
fn registry_compiles_only_requested_language() {
    let reg = registry::LanguageRegistry::new();
    assert_eq!(
        reg.compiled_count(),
        0,
        "a fresh registry must have compiled no languages"
    );
    let rust = LangId::from_name("rust").expect("rust is a known language");
    let result = reg.get(rust);
    assert!(result.is_some(), "rust language must compile successfully");
    assert_eq!(
        reg.compiled_count(),
        1,
        "after requesting rust, exactly one language must be compiled"
    );
}

#[test]
fn highlights_rust_keyword() {
    let result = highlight("rust", "fn main() {}", Duration::from_secs(5)).expect("parse");
    assert!(
        !result.truncated,
        "a trivial source must never hit the span cap"
    );
    let keyword_id = scope_table().resolve("keyword").expect("keyword scope");
    assert!(
        result.spans.iter().any(|(_, id)| *id == keyword_id),
        "expected at least one keyword span, got {:?}",
        result.spans
    );
}

#[test]
fn spans_are_in_painter_order() {
    let result = highlight(
        "rust",
        "fn f(x: u32) -> u32 { x + 1 }",
        Duration::from_secs(5),
    )
    .expect("parse");
    for pair in result.spans.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        assert!(
            a.0.start < b.0.start || (a.0.start == b.0.start && a.0.end >= b.0.end),
            "spans not in painter order: {a:?} before {b:?}"
        );
    }
}

#[test]
fn elapsed_budget_returns_none() {
    let source = "fn f(x: u32) -> u32 { x + 1 }\n".repeat(200);
    assert_eq!(highlight("rust", &source, Duration::ZERO), None);
}

#[test]
fn unbounded_budget_does_not_panic() {
    let result = highlight("rust", "fn main() {}", Duration::MAX);
    assert!(
        result.is_some(),
        "Duration::MAX must be treated as an unbounded budget, not overflow"
    );
}

#[test]
fn unknown_language_returns_none() {
    assert_eq!(highlight("klingon", "x", Duration::from_secs(1)), None);
}

#[test]
fn terraform_query_produces_spans() {
    let source = "resource \"aws_s3_bucket\" \"b\" {\n  bucket = \"x\" # c\n}\n";
    let result = highlight("terraform", source, Duration::from_secs(5)).expect("parse");
    assert!(
        !result.spans.is_empty(),
        "expected at least one terraform span"
    );
}

#[test]
fn truncation_flag_set_when_span_cap_hit() {
    let count = rune_ts::MAX_SPANS + 500;
    let source = format!("[{}]", vec!["1"; count].join(","));
    let result = highlight("json", &source, Duration::from_secs(5)).expect("parse");
    assert_eq!(result.spans.len(), rune_ts::MAX_SPANS);
    assert!(
        result.truncated,
        "collecting past MAX_SPANS must set truncated"
    );
}

#[test]
fn truncation_flag_clear_under_the_cap() {
    let result = highlight("json", "[1, 2, 3]", Duration::from_secs(5)).expect("parse");
    assert!(
        !result.truncated,
        "a small source must never report truncation"
    );
}
