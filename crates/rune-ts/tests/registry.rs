//! Coverage for the compiling half — `registry()` and `highlight()` — plus
//! the compile-free `lang::resolve` paths this package's steps call for.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use rune_syntax::scope::scope_table;
use rune_ts::lang::{self, ALIASES, LANGUAGES};
use rune_ts::{highlight, registry};

#[test]
fn every_language_loads_and_its_query_compiles() {
    let reg = registry();
    assert!(
        reg.failures().is_empty(),
        "language(s) failed to load or compile: {:?}",
        reg.failures()
    );
    assert_eq!(reg.names().count(), 22);
}

#[test]
fn resolves_every_alias() {
    for (alias, name) in ALIASES {
        assert!(
            LANGUAGES.iter().any(|def| def.name == *name),
            "alias {alias:?} points at unknown language {name:?}"
        );
        assert_eq!(
            lang::resolve(alias),
            Some(*name),
            "alias {alias:?} did not resolve to {name:?}"
        );
    }
    assert_eq!(lang::resolve("md"), None);
}

#[test]
fn resolve_touches_no_grammar() {
    assert_eq!(lang::resolve("rs"), Some("rust"));
}

#[test]
fn highlights_rust_keyword() {
    let spans = highlight("rust", "fn main() {}", Duration::from_secs(5)).expect("parse");
    let keyword_id = scope_table().resolve("keyword").expect("keyword scope");
    assert!(
        spans.iter().any(|(_, id)| *id == keyword_id),
        "expected at least one keyword span, got {spans:?}"
    );
}

#[test]
fn spans_are_in_painter_order() {
    let spans = highlight(
        "rust",
        "fn f(x: u32) -> u32 { x + 1 }",
        Duration::from_secs(5),
    )
    .expect("parse");
    for pair in spans.windows(2) {
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
fn unknown_language_returns_none() {
    assert_eq!(highlight("klingon", "x", Duration::from_secs(1)), None);
}

#[test]
fn terraform_query_produces_spans() {
    let source = "resource \"aws_s3_bucket\" \"b\" {\n  bucket = \"x\" # c\n}\n";
    let spans = highlight("terraform", source, Duration::from_secs(5)).expect("parse");
    assert!(!spans.is_empty(), "expected at least one terraform span");
}
