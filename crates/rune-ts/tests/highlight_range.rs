//! Coverage for the parse/query split: `parse` + `highlight_range` over the
//! whole source must reproduce `highlight`'s spans exactly, and scoping the
//! query to a byte range must intersect rather than contain — a construct
//! straddling the range's start still has to paint.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use rune_syntax::scope::scope_table;
use rune_ts::{highlight, highlight_range, parse};

const BUDGET: Duration = Duration::from_secs(5);

/// A dozen lines of real-looking Rust: a doc comment, a struct, a `fn` with
/// a string literal (including one that spans two lines), and a line
/// comment — enough surface for keyword/type/string/comment captures all to
/// fire.
const SOURCE: &str = "\
//! Module doc comment.

struct Point {
    x: i32,
    y: i32,
}

fn greet(name: &str) -> String {
    // Build the greeting.
    let banner = \"hello \\
world\";
    format!(\"{banner} {name}\")
}
";

#[test]
fn highlight_range_full_range_matches_highlight() {
    let whole = highlight("rust", SOURCE, BUDGET).expect("highlight over full source");
    let parsed = parse("rust", SOURCE, BUDGET).expect("parse succeeds");
    let ranged =
        highlight_range(&parsed, 0..SOURCE.len()).expect("highlight_range over full source");

    assert_eq!(
        whole, ranged,
        "highlight_range(0..len) must reproduce highlight()'s spans and painter order exactly"
    );
    assert!(
        !whole.spans.is_empty(),
        "a dozen lines of real Rust source should yield at least one capture"
    );
}

#[test]
fn highlight_range_scopes_to_viewport() {
    let parsed = parse("rust", SOURCE, BUDGET).expect("parse succeeds");
    let whole = highlight_range(&parsed, 0..SOURCE.len()).expect("highlight_range over full");

    // A window over the middle of the source: inside the `greet` function
    // body, starting partway through the multiline string literal that
    // opens with `"hello \` on one line and closes with `world";` on the
    // next. The window's start sits after the string's opening quote, so
    // any span for that string node only "straddles" the window if
    // intersection (not containment) semantics are in effect.
    let window_start = SOURCE.find("world\";").expect("window anchor present");
    let window_end = SOURCE.find("format!").expect("window end anchor present");
    assert!(
        window_start < window_end,
        "window anchors must be in source order"
    );
    let window = window_start..window_end;

    let scoped =
        highlight_range(&parsed, window.clone()).expect("highlight_range over a mid-source window");

    assert!(
        !scoped.spans.is_empty(),
        "the window sits inside a function body and should still yield captures"
    );
    assert!(
        scoped.spans.len() <= whole.spans.len(),
        "scoping to a sub-range must never yield more spans than the full range"
    );

    // Every span the whole-document query found that *intersects* the
    // window must also appear in the scoped result — including the
    // multiline string node that starts before `window_start` (its opening
    // quote is outside the window) but ends inside it.
    let intersects = |r: &std::ops::Range<usize>| r.start < window.end && r.end > window.start;
    for (range, scope_id) in whole.spans.iter().filter(|(range, _)| intersects(range)) {
        assert!(
            scoped
                .spans
                .iter()
                .any(|(r, s)| r == range && s == scope_id),
            "span {range:?} intersects the window and must survive scoping"
        );
    }

    // And nothing entirely outside the window should appear.
    for (range, _) in &scoped.spans {
        assert!(
            intersects(range),
            "span {range:?} does not intersect the requested window {window:?}"
        );
    }

    // The straddling multiline string must specifically be present: some
    // span in the scoped result must start before the window and end
    // inside or after it.
    assert!(
        scoped
            .spans
            .iter()
            .any(|(r, _)| r.start < window.start && r.end > window.start),
        "a construct starting before the window must still be yielded when it intersects it"
    );
}

fn spans_of(lang: &str, source: &str) -> Vec<(std::ops::Range<usize>, rune_syntax::ScopeId)> {
    highlight(lang, source, BUDGET)
        .expect("highlight succeeds")
        .spans
}

#[test]
fn parsed_tree_source_returns_the_exact_input() {
    let source = "fn f() {}";
    let parsed = parse("rust", source, BUDGET).expect("parse succeeds");
    assert_eq!(parsed.source(), source);
}

#[test]
fn rust_string_escape_is_captured() {
    let source = "fn f() { let s = \"a\\nb\"; }";
    let escape = scope_table()
        .resolve("string.escape")
        .expect("string.escape scope");
    let at = source.find("\\n").expect("escape present");
    assert!(
        spans_of("rust", source)
            .iter()
            .any(|(range, id)| *id == escape && range.start == at && range.end == at + 2),
        "the backslash-n escape must carry the string.escape scope"
    );
}

#[test]
fn kotlin_conditional_keyword_is_captured() {
    let source = "if (x) {}";
    let keyword = scope_table().resolve("keyword").expect("keyword scope");
    assert!(
        spans_of("kotlin", source)
            .iter()
            .any(|(range, id)| *id == keyword && range.start == 0 && range.end == 2),
        "kotlin's if must carry the keyword scope"
    );
}

#[test]
fn kotlin_float_literal_is_captured_as_a_number() {
    let source = "val x = 1.5";
    let number = scope_table().resolve("number").expect("number scope");
    let at = source.find("1.5").expect("literal present");
    assert!(
        spans_of("kotlin", source)
            .iter()
            .any(|(range, id)| *id == number && range.start == at && range.end == at + 3),
        "kotlin's float literal must carry the number scope"
    );
}

const MAKE_SOURCE: &str =
    "ifeq ($(OS),Linux)\nCC := gcc\nendif\n\n.PHONY: all\nall: main.o\n\t@echo $@ $(wildcard *.c)\n";

#[test]
fn make_conditional_directive_is_captured() {
    let keyword = scope_table().resolve("keyword").expect("keyword scope");
    assert!(
        spans_of("make", MAKE_SOURCE)
            .iter()
            .any(|(range, id)| *id == keyword && range.start == 0 && range.end == 4),
        "make's ifeq must carry the keyword scope"
    );
}

#[test]
fn make_automatic_variable_is_captured() {
    let variable = scope_table().resolve("variable").expect("variable scope");
    let at = MAKE_SOURCE.find("$@").expect("automatic variable present");
    assert!(
        spans_of("make", MAKE_SOURCE)
            .iter()
            .any(|(range, id)| *id == variable && range.start == at && range.end == at + 2),
        "make's $@ must carry the variable scope"
    );
}

#[test]
fn make_rule_target_is_captured_as_a_function() {
    let function = scope_table().resolve("function").expect("function scope");
    let at = MAKE_SOURCE.find("all:").expect("target present");
    assert!(
        spans_of("make", MAKE_SOURCE)
            .iter()
            .any(|(range, id)| *id == function && range.start == at && range.end == at + 3),
        "make's rule target must carry the function scope"
    );
}

#[test]
fn make_special_target_wins_over_the_plain_target_capture() {
    let builtin = scope_table()
        .resolve("constant.builtin")
        .expect("constant.builtin scope");
    let at = MAKE_SOURCE.find(".PHONY").expect("special target present");
    let spans = spans_of("make", MAKE_SOURCE);
    let last = spans
        .iter()
        .filter(|(range, _)| range.start == at && range.end == at + 6)
        .next_back()
        .expect("a span over .PHONY");
    assert_eq!(
        last.1, builtin,
        "the last span painted over .PHONY must be constant.builtin, not function"
    );
}

const POSTGRES_SOURCE: &str =
    "SELECT id FROM t;\nCREATE FUNCTION f() RETURNS int AS $$body$$ LANGUAGE sql;\n";

#[test]
fn postgres_statement_keyword_is_captured() {
    let keyword = scope_table().resolve("keyword").expect("keyword scope");
    assert!(
        spans_of("postgres", POSTGRES_SOURCE)
            .iter()
            .any(|(range, id)| *id == keyword && range.start == 0 && range.end == 6),
        "postgres's SELECT must carry the keyword scope"
    );
}

#[test]
fn postgres_dollar_quoted_body_is_a_string() {
    let string = scope_table().resolve("string").expect("string scope");
    let at = POSTGRES_SOURCE
        .find("$$body$$")
        .expect("dollar-quoted body present");
    assert!(
        spans_of("postgres", POSTGRES_SOURCE)
            .iter()
            .any(|(range, id)| *id == string && range.start == at && range.end == at + 8),
        "postgres's dollar-quoted body must carry the string scope"
    );
}

#[test]
fn plpgsql_control_flow_keywords_are_captured() {
    let source = "BEGIN\n  IF x THEN\n    RAISE NOTICE 'hi';\n  END IF;\nEND\n";
    let keyword = scope_table().resolve("keyword").expect("keyword scope");
    let spans = spans_of("plpgsql", source);
    let at_if = source.find("IF").expect("IF present");
    let at_raise = source.find("RAISE").expect("RAISE present");
    assert!(
        spans
            .iter()
            .any(|(range, id)| *id == keyword && range.start == at_if && range.end == at_if + 2),
        "plpgsql's IF must carry the keyword scope"
    );
    assert!(
        spans.iter().any(
            |(range, id)| *id == keyword && range.start == at_raise && range.end == at_raise + 5
        ),
        "plpgsql's RAISE must carry the keyword scope"
    );
}
