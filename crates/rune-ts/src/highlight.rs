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

use std::ops::{ControlFlow, Range};
use std::sync::{Arc, LazyLock};
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

// Grammar crates ship Neovim-convention capture names the closed scope
// vocabulary never registers; each one maps onto the scope it means.
const CAPTURE_ALIASES: &[(&str, &str)] = &[
    ("escape", "string.escape"),
    ("module", "type"),
    ("module.builtin", "type.builtin"),
    ("namespace", "type"),
    ("parameter", "variable.parameter"),
    ("field", "variable.member"),
    ("float", "number"),
    ("conditional", "keyword"),
    ("repeat", "keyword"),
    ("exception", "keyword"),
    ("include", "keyword"),
    ("storageclass", "keyword"),
    ("character", "string"),
    ("character.special", "string.escape"),
    ("delimiter", "punctuation.delimiter"),
];

// Deliberately unstyled: spell is a spellcheck hint, embedded marks an
// injected-language region, none is an explicit no-style marker, and a
// leading `_` marks a query-internal capture.
const IGNORED_CAPTURES: &[&str] = &["spell", "embedded", "none"];

enum CaptureResolution {
    Scope(ScopeId),
    Ignored,
    Unknown,
}

fn classify_capture(name: &str) -> CaptureResolution {
    if let Some(id) = SCOPES.resolve(name) {
        return CaptureResolution::Scope(id);
    }
    if name.starts_with('_') {
        return CaptureResolution::Ignored;
    }
    let mut candidate = name;
    loop {
        if let Some((_, target)) = CAPTURE_ALIASES
            .iter()
            .find(|(alias, _)| *alias == candidate)
        {
            return match SCOPES.resolve(target) {
                Some(id) => CaptureResolution::Scope(id),
                None => CaptureResolution::Unknown,
            };
        }
        if IGNORED_CAPTURES.contains(&candidate) {
            return CaptureResolution::Ignored;
        }
        let Some(pos) = candidate.rfind('.') else {
            return CaptureResolution::Unknown;
        };
        let Some(next) = candidate.get(..pos) else {
            return CaptureResolution::Unknown;
        };
        candidate = next;
    }
}

pub fn declared_capture_names() -> impl Iterator<Item = &'static str> {
    CAPTURE_ALIASES
        .iter()
        .map(|(alias, _)| *alias)
        .chain(IGNORED_CAPTURES.iter().copied())
}

pub fn capture_is_accounted_for(name: &str) -> bool {
    !matches!(classify_capture(name), CaptureResolution::Unknown)
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
        let resolution = classify_capture(capture_name);
        assert_invariant!(
            !matches!(resolution, CaptureResolution::Unknown),
            || format!(
                "unresolved tree-sitter capture @{capture_name} for language {}: not a \
                 registered scope, not in CAPTURE_ALIASES, and not in IGNORED_CAPTURES",
                lang.name()
            )
        );
        let CaptureResolution::Scope(scope_id) = resolution else {
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
mod tests {
    use super::*;

    #[test]
    fn every_capture_alias_targets_a_registered_scope() {
        for (alias, target) in CAPTURE_ALIASES {
            assert!(
                SCOPES.iter().any(|(_, name)| name == *target),
                "CAPTURE_ALIASES maps @{alias} to {target}, which is not a registered scope name"
            );
        }
    }

    #[test]
    fn no_capture_alias_key_is_already_reachable() {
        for (alias, _) in CAPTURE_ALIASES {
            assert!(
                SCOPES.resolve(alias).is_none(),
                "CAPTURE_ALIASES key @{alias} already resolves on its own, so its entry is dead"
            );
        }
    }
}
