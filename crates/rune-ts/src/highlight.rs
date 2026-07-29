//! Turns `(language, source)` into painter-ordered, scope-resolved spans.
//! The only function in this crate that constructs a `Parser` or runs a
//! `Query` — reaching it means touching `registry()`, so callers must only
//! ever call it from a background command thread, never the UI thread.

use std::ops::{ControlFlow, Range};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use rune_syntax::scope::scope_table;
use rune_syntax::{ScopeId, ScopeTable};
use tree_sitter::{ParseOptions, ParseState, Parser, QueryCursor, StreamingIterator};

use crate::lang;
use crate::registry::registry;

/// A parse never yields more spans than this — bounds both the memory a
/// single highlight reply can hold and the time the render-layer overlay
/// spends painting it.
pub const MAX_SPANS: usize = 100_000;

/// The scope table every capture name is resolved against — built from the
/// same [`scope_table`] constructor `rune-md`'s emitter and `rune-tui`'s
/// theme use, so a `ScopeId` this crate hands out means the same thing to
/// both.
static SCOPES: LazyLock<ScopeTable> = LazyLock::new(scope_table);

/// One [`highlight`] call's outcome: its spans in painter order, plus
/// whether the query still had unyielded captures when collection stopped
/// at [`MAX_SPANS`] — the flag that lets a caller tell a truncated result
/// from a complete one instead of the two being indistinguishable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HighlightResult {
    pub spans: Vec<(Range<usize>, ScopeId)>,
    pub truncated: bool,
}

impl From<Vec<(Range<usize>, ScopeId)>> for HighlightResult {
    /// A complete result — nothing was dropped to the span cap. The natural
    /// reading of a span list handed over directly rather than produced by a
    /// capped parse.
    fn from(spans: Vec<(Range<usize>, ScopeId)>) -> HighlightResult {
        HighlightResult {
            spans,
            truncated: false,
        }
    }
}

/// Parses `source` as `lang` and returns its highlight spans in painter
/// order: `(range.start ASC, range.end DESC, yield-order ASC)`, so an
/// enclosing capture always comes before one it contains and an earlier
/// query pattern before a later one over the same node — reproducing
/// `tree-sitter-highlight`'s innermost-and-last-wins resolution with no
/// per-cell search. [`HighlightResult::truncated`] is set when the cap in
/// [`MAX_SPANS`] was reached before the query finished yielding — the cap
/// itself always stays in force, this only makes hitting it observable.
///
/// `None` means no result — an unrecognised language, a `Parser::set_language`
/// failure recorded in `registry().failures()`, or a parse that did not
/// finish inside `budget` — never a panic. Callers must treat `None` as "no
/// new information" and keep whatever spans they already had, not erase
/// them.
///
/// Always a full parse — no retained tree to reparse incrementally from.
/// Callers that highlight the SAME document repeatedly (the whole-buffer
/// path a typed keystroke re-triggers) should use [`Reparser`] instead; this
/// function stays for one-shot callers with no document identity to key a
/// retained tree on (each fence in a markdown document is reparsed fresh
/// every time regardless, since a fence's reconstructed source is rebuilt
/// from scratch on every call anyway).
pub fn highlight(lang: &str, source: &str, budget: Duration) -> Option<HighlightResult> {
    let name = lang::resolve(lang)?;
    let (language, query) = registry().get(name)?;

    let mut parser = Parser::new();
    parser.set_language(language).ok()?;

    let tree = parse(&mut parser, source, budget, None)?;
    Some(spans_from_tree(&tree, query, source.as_bytes()))
}

/// Per-document incremental-reparse state (plan WP16.S3): retains the
/// tree-sitter `Tree` and source text the last successful [`Reparser::
/// highlight`] call built, so the NEXT call — if it names the same language
/// — can feed tree-sitter a single `InputEdit` (the common-prefix/suffix
/// diff against the retained source) instead of parsing from scratch.
/// `HIGHLIGHT_BUDGET`/[`MAX_SPANS`] stay in force exactly as they do for the
/// free [`highlight`] function; incremental reparse only changes how much
/// WORK a parse within that budget has to redo, never the budget or cap
/// themselves. One `Reparser` belongs to exactly one document — mixing
/// documents (or a document that changes language) through the same
/// instance just falls back to a full parse on the mismatch, it never
/// produces a wrong result.
#[derive(Debug, Default)]
pub struct Reparser {
    tree: Option<tree_sitter::Tree>,
    source: String,
    lang: Option<&'static str>,
}

impl Reparser {
    pub fn new() -> Reparser {
        Reparser::default()
    }

    /// Same contract as [`highlight`] (parses `source` as `lang` within
    /// `budget`, `None` on an unrecognised language/query-compile failure/
    /// timeout), but reuses the previous call's retained tree as the
    /// reparse base when `lang` is unchanged and a diff against the
    /// previous source can be computed — tree-sitter then only re-walks
    /// the nodes the edit actually touched. Falls back to a full parse
    /// (tree-sitter's own `old_tree: None` contract) on the first call, a
    /// language change, or an identical source (`diff_edit` returning
    /// `None`, nothing to feed as an edit). A timed-out call leaves the
    /// retained tree/source as they were: the failed attempt never partakes
    /// in a LATER call's diff, so one slow parse degrades that one call
    /// only, not every call after it.
    pub fn highlight(&mut self, lang: &str, source: &str, budget: Duration) -> Option<HighlightResult> {
        let name = lang::resolve(lang)?;
        let (language, query) = registry().get(name)?;

        let mut parser = Parser::new();
        parser.set_language(language).ok()?;

        let old_tree = (self.lang == Some(name))
            .then(|| self.tree.take())
            .flatten()
            .and_then(|mut tree| {
                let edit = diff_edit(&self.source, source)?;
                tree.edit(&edit);
                Some(tree)
            });

        let tree = parse(&mut parser, source, budget, old_tree.as_ref())?;
        let result = spans_from_tree(&tree, query, source.as_bytes());
        self.tree = Some(tree);
        self.source = source.to_string();
        self.lang = Some(name);
        Some(result)
    }
}

/// The parse call itself, shared by [`highlight`] and [`Reparser::
/// highlight`] — the only difference between a full and an incremental
/// parse is whether `old_tree` is `Some`.
fn parse(
    parser: &mut Parser,
    source: &str,
    budget: Duration,
    old_tree: Option<&tree_sitter::Tree>,
) -> Option<tree_sitter::Tree> {
    // `Instant + Duration` panics on overflow; a public function taking an
    // arbitrary caller-supplied `Duration` must not trust it to stay in
    // range. `checked_add` returning `None` (a `budget` so large the
    // deadline can't be represented) is treated as no deadline at all —
    // the honest reading of an effectively unbounded budget — rather than
    // a reason to fail the parse.
    let deadline = Instant::now().checked_add(budget);
    let mut on_progress = |_: &ParseState| {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };

    let bytes = source.as_bytes();
    parser.parse_with_options(
        // The tail slice at the given offset, never the whole buffer — see
        // this crate's module docs on why the callback must be shaped this
        // way. `.get` keeps this clear of the `indexing_slicing` lint and
        // `unwrap_or_default` yields the empty slice the end-of-input
        // contract requires, without being the denied `unwrap_used`.
        &mut |i, _| bytes.get(i..).unwrap_or_default(),
        old_tree,
        Some(ParseOptions::new().progress_callback(&mut on_progress)),
    )
}

/// Walks `tree`'s query captures into painter-ordered, scope-resolved spans
/// — the tail half of both [`highlight`] and [`Reparser::highlight`], once
/// each has its own `Tree` (a fresh one or an incrementally reparsed one).
fn spans_from_tree(tree: &tree_sitter::Tree, query: &tree_sitter::Query, bytes: &[u8]) -> HighlightResult {
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(query, tree.root_node(), bytes);
    let mut spans: Vec<(Range<usize>, ScopeId, usize)> = Vec::new();
    let mut seq: usize = 0;
    let mut truncated = false;
    while let Some((query_match, capture_idx)) = captures.next() {
        let Some(capture) = query_match.captures.get(*capture_idx) else {
            continue;
        };
        let Some(capture_name) = query.capture_names().get(capture.index as usize) else {
            continue;
        };
        let Some(scope_id) = SCOPES.resolve(capture_name) else {
            continue;
        };
        let range = capture.node.byte_range();
        if range.start >= range.end {
            continue;
        }
        spans.push((range, scope_id, seq));
        seq += 1;
        if spans.len() >= MAX_SPANS {
            truncated = true;
            break;
        }
    }

    spans.sort_by(|a, b| {
        a.0.start
            .cmp(&b.0.start)
            .then(b.0.end.cmp(&a.0.end))
            .then(a.2.cmp(&b.2))
    });

    HighlightResult {
        spans: spans
            .into_iter()
            .map(|(range, scope_id, _)| (range, scope_id))
            .collect(),
        truncated,
    }
}

/// The single `InputEdit` tree-sitter needs to reparse incrementally,
/// derived from the common byte prefix/suffix between `old` and `new` —
/// `Reparser` has no access to the actual edit the user made (only the
/// before/after whole-document text), so this reconstructs an equivalent
/// edit rather than requiring one to be threaded through from the buffer's
/// own edit machinery. `None` when `old == new` (nothing changed, no edit
/// to feed). Byte-based throughout, never `char`-based: tree-sitter's
/// `InputEdit`/`Point` are byte offsets, and prefix/suffix bytes are never
/// sliced back out as `str`, so no UTF-8 boundary requirement applies.
fn diff_edit(old: &str, new: &str) -> Option<tree_sitter::InputEdit> {
    if old == new {
        return None;
    }
    let old_bytes = old.as_bytes();
    let new_bytes = new.as_bytes();
    let max_common = old_bytes.len().min(new_bytes.len());

    // Iterator-driven, never indexed: `zip` alone already bounds this to
    // `max_common`, and `take_while` stops at the first mismatching byte.
    let prefix = old_bytes
        .iter()
        .zip(new_bytes.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let max_suffix = max_common - prefix;
    let suffix = old_bytes
        .iter()
        .rev()
        .zip(new_bytes.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();

    let start_byte = prefix;
    let old_end_byte = old_bytes.len() - suffix;
    let new_end_byte = new_bytes.len() - suffix;

    Some(tree_sitter::InputEdit {
        start_byte,
        old_end_byte,
        new_end_byte,
        start_position: point_at(old, start_byte),
        old_end_position: point_at(old, old_end_byte),
        new_end_position: point_at(new, new_end_byte),
    })
}

/// The `tree_sitter::Point` (row, byte-column) of byte offset `byte` in
/// `text` — a plain linear scan over `text[..byte]`, bounded by the edit
/// position rather than the document length in the common case (an edit
/// near the start or end of a large document), and cheap regardless
/// compared to the parse it feeds into.
fn point_at(text: &str, byte: usize) -> tree_sitter::Point {
    let mut row = 0usize;
    let mut last_newline: Option<usize> = None;
    // `.get(..byte)` degrades to the whole slice rather than panicking if
    // `byte` somehow exceeds `text`'s length (never expected — every caller
    // passes an offset derived from `old`/`new` themselves).
    for (i, b) in text
        .as_bytes()
        .get(..byte)
        .unwrap_or(text.as_bytes())
        .iter()
        .enumerate()
    {
        if *b == b'\n' {
            row += 1;
            last_newline = Some(i);
        }
    }
    let column = match last_newline {
        Some(nl) => byte - nl - 1,
        None => byte,
    };
    tree_sitter::Point { row, column }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const BUDGET: Duration = Duration::from_secs(5);

    #[test]
    fn reparser_first_call_is_a_full_parse_and_matches_the_free_function() {
        let source = "fn main() {\n    let a = 1;\n}\n";
        let mut reparser = Reparser::new();
        let incremental = reparser
            .highlight("rust", source, BUDGET)
            .expect("trivial rust source must parse");
        let full = highlight("rust", source, BUDGET).expect("trivial rust source must parse");
        assert_eq!(incremental, full);
    }

    #[test]
    fn reparser_produces_the_same_spans_as_a_full_parse_after_an_edit() {
        let before = "fn main() {\n    let a = 1;\n}\n";
        let after = "fn main() {\n    let abcde = 1;\n}\n";

        let mut reparser = Reparser::new();
        reparser
            .highlight("rust", before, BUDGET)
            .expect("first parse must succeed");
        let incremental = reparser
            .highlight("rust", after, BUDGET)
            .expect("incremental reparse must succeed");

        let full = highlight("rust", after, BUDGET).expect("full parse of the edited text");
        assert_eq!(
            incremental, full,
            "an incremental reparse must produce identical spans to a full parse of the same text"
        );
    }

    #[test]
    fn reparser_falls_back_to_a_full_parse_on_a_language_change() {
        let rust_source = "fn main() {}\n";
        let json_source = "{\"a\": 1}\n";

        let mut reparser = Reparser::new();
        reparser
            .highlight("rust", rust_source, BUDGET)
            .expect("rust parse must succeed");
        let switched = reparser
            .highlight("json", json_source, BUDGET)
            .expect("json parse must succeed after a language switch");

        let full = highlight("json", json_source, BUDGET).expect("full json parse");
        assert_eq!(switched, full);
    }

    #[test]
    fn reparser_handles_repeated_edits_at_growing_lengths() {
        // A closer approximation of real typing: the same document, edited
        // several times in a row, each building on the previous retained
        // tree — not just a single before/after pair.
        let mut reparser = Reparser::new();
        let mut source = String::from("fn main() {\n    let a = 1;\n}\n");
        let mut last = None;
        for extra in ["a", "b", "c", "d"] {
            // Insert right after the stable "let " prefix each time, so the
            // anchor never depends on text a previous iteration changed.
            let at = source.find("let ").expect("marker") + "let ".len();
            source.insert_str(at, extra);
            last = Some(
                reparser
                    .highlight("rust", &source, BUDGET)
                    .expect("each incremental step must succeed"),
            );
        }
        let full = highlight("rust", &source, BUDGET).expect("full parse of the final text");
        assert_eq!(last.expect("looped at least once"), full);
    }

    #[test]
    fn diff_edit_is_none_for_identical_text() {
        assert!(diff_edit("same text\n", "same text\n").is_none());
    }

    #[test]
    fn diff_edit_finds_the_minimal_common_prefix_and_suffix() {
        let old = "abcXYZdef";
        let new = "abc123456def";
        let edit = diff_edit(old, new).expect("texts differ");
        assert_eq!(edit.start_byte, 3);
        assert_eq!(edit.old_end_byte, 6); // "XYZ" ends at byte 6
        assert_eq!(edit.new_end_byte, 9); // "123456" ends at byte 9
    }
}
