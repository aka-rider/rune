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
/// Every call is a full parse — incremental reparse is never attempted.
pub fn highlight(lang: &str, source: &str, budget: Duration) -> Option<HighlightResult> {
    let name = lang::resolve(lang)?;
    let (language, query) = registry().get(name)?;

    let mut parser = Parser::new();
    parser.set_language(language).ok()?;

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
    let tree = parser.parse_with_options(
        // The tail slice at the given offset, never the whole buffer — see
        // this crate's module docs on why the callback must be shaped this
        // way. `.get` keeps this clear of the `indexing_slicing` lint and
        // `unwrap_or_default` yields the empty slice the end-of-input
        // contract requires, without being the denied `unwrap_used`.
        &mut |i, _| bytes.get(i..).unwrap_or_default(),
        None,
        Some(ParseOptions::new().progress_callback(&mut on_progress)),
    )?;

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

    Some(HighlightResult {
        spans: spans
            .into_iter()
            .map(|(range, scope_id, _)| (range, scope_id))
            .collect(),
        truncated,
    })
}
