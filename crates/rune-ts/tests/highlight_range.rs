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
