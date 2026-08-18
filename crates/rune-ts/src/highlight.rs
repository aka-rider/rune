//! Turns `(language, source)` into painter-ordered, scope-resolved spans.
//! The only module in this crate that constructs a `Parser` or runs a
//! `Query` — reaching either half of it means touching `registry()`, which
//! compiles the requested language's query on first use.
//!
//! The two halves have different cost profiles and belong on different
//! threads. [`parse`] runs a whole-document tree-sitter parse plus (on
//! first use of that language) a query compile; it belongs on a background
//! command thread, with exactly one sanctioned exception: a single bounded
//! attempt before the first frame is drawn, made where that startup call
//! site lives, while nothing is on screen yet to block. [`highlight_range`]
//! only walks an already-parsed [`ParsedTree`] with a query restricted to a
//! byte range — no parsing, no query compile — and is cheap enough to run
//! on the render path once per frame, scoped to whatever byte range is
//! currently visible.

use std::collections::HashSet;
use std::ops::{ControlFlow, Range};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use rune_core::assert_invariant;
use rune_syntax::scope::scope_table;
use rune_syntax::{LangId, ScopeId, ScopeTable};
use tree_sitter::{ParseOptions, ParseState, Parser, Query, QueryCursor, StreamingIterator, Tree};

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

/// Capture names a grammar's bundled query already used before this
/// mechanism existed, that the closed scope table has no entry (or
/// dotted-prefix ancestor) for. [`note_unresolved_capture`] records every
/// occurrence in [`UNRESOLVED_CAPTURES`] regardless, but only asserts on a
/// pair that is NOT here — real source text hits this debt constantly (an
/// `@escape` inside almost any string literal, for instance), so treating
/// every occurrence as a hard failure would turn ordinary highlighting into
/// a crash. Extending the closed vocabulary to cover one of these is a
/// theming decision (it also needs a `Style` wherever `rune-tui`'s theme
/// builds its `scopes` vector), not a mechanical one — deleting an entry
/// here without doing that work would silently regress highlighting for
/// whatever capture it names.
pub const KNOWN_UNRESOLVED_CAPTURES: &[(&str, &str)] = &[
    ("rust", "escape"),
    ("json", "escape"),
    ("bash", "embedded"),
    ("python", "embedded"),
    ("python", "escape"),
    ("javascript", "embedded"),
    ("go", "escape"),
    ("c", "delimiter"),
    ("typescript", "embedded"),
    ("tsx", "embedded"),
    ("csharp", "module"),
    ("php", "module"),
    ("php", "module.builtin"),
    ("ruby", "embedded"),
    ("ruby", "escape"),
    ("sql", "conditional"),
    ("sql", "field"),
    ("sql", "float"),
    ("sql", "parameter"),
    ("sql", "spell"),
    ("sql", "storageclass"),
    ("kotlin", "_class"),
    ("kotlin", "_function"),
    ("kotlin", "character"),
    ("kotlin", "conditional"),
    ("kotlin", "exception"),
    ("kotlin", "float"),
    ("kotlin", "include"),
    ("kotlin", "namespace"),
    ("kotlin", "none"),
    ("kotlin", "parameter"),
    ("kotlin", "repeat"),
    ("swift", "character.special"),
    ("swift", "spell"),
];

/// Every `(language, capture name)` pair already reported by
/// [`note_unresolved_capture`] — the set that turns a per-keystroke flood
/// into a one-time signal. A grammar's query is fixed for the process's
/// lifetime (compiled once by `registry()` and never recompiled), so a
/// capture that fails to resolve today will fail identically on every later
/// call; recording it once is exactly as informative as recording it every
/// time and immeasurably cheaper.
static UNRESOLVED_CAPTURES: LazyLock<Mutex<HashSet<(LangId, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Called whenever a query capture name resolves to `None` against
/// [`SCOPES`] — a query (whether bundled by a grammar crate or hand-authored
/// in this crate's `queries/`) using a scope outside the closed vocabulary
/// [`rune_syntax::scope`] defines. The guard test over every
/// [`crate::lang::LANGUAGES`] entry's compiled query exists to catch a
/// brand-new instance of this before a grammar bump ships one.
///
/// The highlight path runs on a background thread at keystroke rate, so
/// this cannot log or panic unconditionally without either adding a logging
/// dependency this crate has never needed or turning routine highlighting
/// of pre-existing debt into a crash loop. Instead: every occurrence is
/// recorded once per `(language, name)` pair in [`UNRESOLVED_CAPTURES`], so
/// a caller (a test, or future diagnostics plumbing) can observe that it
/// happened without polling a log — and the first occurrence of a pair
/// outside [`KNOWN_UNRESOLVED_CAPTURES`] also trips [`assert_invariant`], a
/// hard failure under `cargo test` and any `strict-invariants` build,
/// silent in an ordinary release build.
fn note_unresolved_capture(lang: LangId, name: &str) {
    let mut seen = UNRESOLVED_CAPTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if seen.insert((lang, name.to_string())) {
        let known = KNOWN_UNRESOLVED_CAPTURES.contains(&(lang.name(), name));
        assert_invariant!(known, || format!(
            "unresolved tree-sitter capture @{name} for language {:?}: the \
             closed ScopeTable vocabulary has no entry (or dotted prefix) \
             for it, and it is not in KNOWN_UNRESOLVED_CAPTURES",
            lang.name()
        ));
    }
}

/// The `(language, capture name)` pairs seen so far by
/// [`note_unresolved_capture`] — every capture, once ever, that a query
/// asked [`SCOPES`] to resolve and got `None` back. Exposed for tests: the
/// production highlight path only ever inserts into this set, never reads
/// it back.
#[cfg(test)]
pub(crate) fn unresolved_captures() -> HashSet<(LangId, String)> {
    UNRESOLVED_CAPTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// One [`highlight`]/[`highlight_range`] call's outcome: its spans in
/// painter order, plus whether the query still had unyielded captures when
/// collection stopped at [`MAX_SPANS`] — the flag that lets a caller tell a
/// truncated result from a complete one instead of the two being
/// indistinguishable.
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

/// A whole-document parse retained across frames so later queries never
/// reparse. Holds the tree itself, the canonical language name it was
/// resolved to (so a repeated per-frame query never re-resolves a name or
/// allocates a lowercase copy), and the exact source text the tree was
/// parsed from.
///
/// Retaining that source is load-bearing, not an optimisation: query
/// predicates such as `#eq?`/`#match?` evaluate node text against the bytes
/// the tree was built from. Querying a retained tree against a possibly
/// newer live buffer would resolve those predicates against bytes that no
/// longer correspond to the tree's node boundaries. Every query in this
/// module runs against the retained snapshot, never the live buffer;
/// whatever offset drift accumulates between a parse and the next reparse
/// landing is the same staleness class the rest of the highlight pipeline
/// already tolerates, and is inert at paint time (a stale span simply
/// matches no cell it wasn't meant to).
#[derive(Debug)]
pub struct ParsedTree {
    tree: Tree,
    lang: LangId,
    source: Arc<str>,
}

impl ParsedTree {
    /// The exact source text this tree was parsed from.
    ///
    /// Exposed so a caller holding a retained tree can decide whether it
    /// still describes the text it would otherwise be about to reparse:
    /// equality against a freshly reconstructed source is the whole
    /// validity test, since every parse here is a full parse of exactly
    /// these bytes.
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Parses `source` as `lang` from scratch and returns the retained tree, or
/// `None` for an unrecognised language, a `Parser::set_language`/query
/// compile failure recorded in `registry().failures()`, or a parse that did
/// not finish inside `budget` — never a panic.
///
/// Every call is a full parse — incremental reparse (`Tree::edit`) is never
/// attempted; the grammar-crash risk that guards against is not worth the
/// saved cycles.
pub fn parse(lang: &str, source: &str, budget: Duration) -> Option<ParsedTree> {
    let id = lang::resolve(lang)?;
    let (language, _query) = registry().get(id)?;

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
        // this module's docs on why the callback must be shaped this way.
        // `.get` keeps this clear of the `indexing_slicing` lint and
        // `unwrap_or_default` yields the empty slice the end-of-input
        // contract requires, without being the denied `unwrap_used`.
        &mut |i, _| bytes.get(i..).unwrap_or_default(),
        None,
        Some(ParseOptions::new().progress_callback(&mut on_progress)),
    )?;

    Some(ParsedTree {
        tree,
        lang: id,
        source: Arc::from(source),
    })
}

/// Runs `parsed`'s query restricted to `range` and returns the matching
/// spans in painter order, exactly as [`highlight`] would for the same
/// tree — the only difference is that captures whose node does not
/// intersect `range` are never visited.
///
/// `range` is a byte-range *intersection* filter, not a containment one: a
/// multiline node that starts before `range` and ends inside or after it is
/// still yielded in full, because the part of it inside `range` still needs
/// to paint. Callers must not treat a returned span's bounds as clipped to
/// `range`.
///
/// `None` means the language is no longer resolvable against `registry()` —
/// this should not happen for a `parsed` produced by [`parse`], since the
/// same registry backs both, but is returned rather than assumed.
pub fn highlight_range(parsed: &ParsedTree, range: Range<usize>) -> Option<HighlightResult> {
    let (_language, query) = registry().get(parsed.lang)?;
    Some(run_query(
        parsed.lang,
        query,
        &parsed.tree,
        &parsed.source,
        Some(range),
    ))
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
/// Equivalent to [`parse`] followed by [`highlight_range`] over the whole
/// source; kept as its own entry point because most fence-sized documents
/// never need the retained tree past a single call.
pub fn highlight(lang: &str, source: &str, budget: Duration) -> Option<HighlightResult> {
    let parsed = parse(lang, source, budget)?;
    let len = parsed.source.len();
    highlight_range(&parsed, 0..len)
}

/// The query/capture/collect loop shared by [`highlight_range`] and
/// [`highlight`]: walks every capture over `tree` (restricted to `range`
/// when given one), resolves each capture name against [`SCOPES`], and
/// returns the results in the one painter-order sort this crate ever
/// applies.
fn run_query(
    lang: LangId,
    query: &Query,
    tree: &Tree,
    source: &str,
    range: Option<Range<usize>>,
) -> HighlightResult {
    let bytes = source.as_bytes();
    let mut cursor = QueryCursor::new();
    if let Some(range) = range {
        cursor.set_byte_range(range);
    }
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
            note_unresolved_capture(lang, capture_name);
            continue;
        };
        let node_range = capture.node.byte_range();
        if node_range.start >= node_range.end {
            continue;
        }
        spans.push((node_range, scope_id, seq));
        seq += 1;
        if spans.len() >= MAX_SPANS {
            truncated = true;
            break;
        }
    }

    spans.sort_by(painter_order);

    HighlightResult {
        spans: spans
            .into_iter()
            .map(|(range, scope_id, _)| (range, scope_id))
            .collect(),
        truncated,
    }
}

/// The one comparator behind the painter-order contract documented on
/// [`highlight`]: `(range.start ASC, range.end DESC, yield-order ASC)`.
/// Exists exactly once so [`highlight_range`] and [`highlight`] can never
/// drift apart on ordering.
fn painter_order(
    a: &(Range<usize>, ScopeId, usize),
    b: &(Range<usize>, ScopeId, usize),
) -> std::cmp::Ordering {
    a.0.start
        .cmp(&b.0.start)
        .then(b.0.end.cmp(&a.0.end))
        .then(a.2.cmp(&b.2))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_known_unresolved_capture_is_recorded_without_panicking() {
        let rust = LangId::from_name("rust").expect("rust is a known LangId");
        let source = r#"fn main() { let _s = "a\nb"; }"#;
        let _ = highlight(rust.name(), source, Duration::from_secs(1));
        assert!(unresolved_captures().contains(&(rust, "escape".to_string())));
    }
}
