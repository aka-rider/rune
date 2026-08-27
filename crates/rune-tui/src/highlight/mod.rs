pub mod query;

use std::ops::Range;

use rune_md::element::code_region::CodeRegion;
use rune_syntax::ScopeId;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::linemap::LineMap;
use crate::runtime::{self, Effects};

pub use query::visible_spans;

// At most one highlight in flight per document — `in_flight` gates a second
// dispatch into `pending` instead, since there is no thread pool and no
// cancellation for a running pass.
#[derive(Debug, Default)]
pub struct HighlightState {
    pub version: u64,
    pub regions: Vec<RegionHighlight>,
    pub in_flight: Option<u64>,
    pub pending: bool,
    // Read back after storing a reply to drive the status line — unless
    // that same reply also timed out, in which case the timeout takes
    // precedence and is shown instead.
    pub truncated: bool,
}

// A reply populates exactly one of `tree`/`spans` and clears the other; the
// render path reads both, so neither is silently ignored.
#[derive(Debug, Default)]
pub struct RegionHighlight {
    pub map: LineMap,
    pub source: String,
    pub tree: Option<rune_ts::ParsedTree>,
    pub spans: Vec<(Range<usize>, ScopeId)>,
}

#[derive(Debug)]
pub enum RegionPayload {
    Tree(rune_ts::ParsedTree),
    // Buffer-coordinate spans, already mapped back through the region's own
    // `LineMap` — a nested fence's container prefix is excluded by
    // construction.
    Spans {
        source: String,
        spans: Vec<(Range<usize>, ScopeId)>,
    },
}

// `CarryForward` carries the region's reconstructed source text — the key
// stored channels are matched against, so colours survive only when
// produced from these exact bytes, never merely by sitting at the same
// index.
#[derive(Debug)]
pub enum RegionOutcome {
    CarryForward { source: String },
    Replace(RegionPayload),
}

#[derive(Debug)]
pub struct RegionResult {
    pub map: LineMap,
    pub outcome: RegionOutcome,
}

// `CarryForward` means no region produced anything and must leave every
// stored region untouched: a slow document degrades to stale colours,
// never to none — distinct from a `Replace` whose regions all carry
// forward individually.
#[derive(Debug)]
pub enum PassOutcome {
    CarryForward,
    Replace(HighlightReply),
}

// Applying a reply is a total replacement of `regions`, never a patch — so
// no index in it can ever refer to a region that does not exist.
#[derive(Debug)]
pub struct HighlightReply {
    pub regions: Vec<RegionResult>,
    pub truncated: bool,
}

pub(crate) struct RegionJob {
    pub(crate) map: LineMap,
    pub(crate) work: RegionWork,
}

pub(crate) enum RegionWork {
    Retain { source: String },
    Parse { lang: RegionLang, source: String },
}

// Markdown's two spellings are never registered in `rune_ts::lang::resolve`'s
// own registry, so the two resolution paths are mutually exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RegionLang {
    Ts(&'static str),
    Markdown,
}

struct RegionSource {
    lang: RegionLang,
    map: LineMap,
    text: String,
}

// The first token after splitting on whitespace and `,` (a fence may be
// tagged ```rust,ignore``` or ```rust title=x```). An unresolved token
// contributes nothing and is not an error — a region still needs to exist
// because a consumer painting a background still cares about it.
fn region_language(info: &str) -> Option<RegionLang> {
    let token = info
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|s| !s.is_empty())?;
    if token.eq_ignore_ascii_case("markdown") || token.eq_ignore_ascii_case("md") {
        return Some(RegionLang::Markdown);
    }
    rune_ts::lang::resolve(token).map(|id| RegionLang::Ts(id.name()))
}

// A region's `content` is one `Range` per physical content line — for a
// fence nested inside a blockquote or list item, the gap between two
// consecutive lines' buffer ranges holds that container's own repeating
// prefix (`"> "`, a list marker's indent), which must never reach a parser
// as source bytes: tree-sitter's error recovery silently absorbs a stray
// prefix for some grammars, but an indentation-sensitive one loses most of
// its structure to it.
fn region_sources(content: &str, regions: &[CodeRegion]) -> Vec<RegionSource> {
    regions
        .iter()
        .filter_map(|region| {
            let lang = region_language(&region.info)?;
            let map = LineMap::new(content, region.content.clone());
            // Should never fail — the ranges are derived from the buffer's
            // own parse — but a region that somehow doesn't land on a live
            // byte range is skipped rather than causing a panic.
            let text = map.reconstruct(content)?;
            Some(RegionSource { lang, map, text })
        })
        .collect()
}

// The rebuild must happen before any region range is read: the settle step
// that normally rebuilds it runs AFTER the update loop returns, so without
// this call the regions would describe the previous buffer version while
// the command gets stamped with the current one — a reply the version
// check would then accept as authoritative, painting every region at a
// shifted offset until the next keystroke.
fn resolve_region_sources(app: &mut App, id: DocumentId) -> Vec<RegionSource> {
    let Some(doc) = app.doc_mut(id) else {
        return Vec::new();
    };
    let view = doc.view();
    #[cfg(test)]
    test_support::record_resolve_call();
    region_sources(doc.buffer.content(), &view.code_regions)
}

#[cfg(test)]
mod test_support {
    use std::cell::Cell;

    // Thread-local rather than a struct field: the default test harness
    // gives each `#[test]` its own thread, so every test starts counting
    // from zero regardless of test ordering.
    thread_local! {
        static RESOLVE_CALLS: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn record_resolve_call() {
        RESOLVE_CALLS.with(|c| c.set(c.get() + 1));
    }

    pub(super) fn resolve_call_count() -> usize {
        RESOLVE_CALLS.with(Cell::get)
    }
}

// The one writer of `HighlightState::regions`.
fn install_regions(doc: &mut Document, version: u64, reply: HighlightReply) {
    let mut carried = std::mem::take(&mut doc.highlight.regions);
    doc.highlight.regions = reply
        .regions
        .into_iter()
        .enumerate()
        .map(|(i, result)| match result.outcome {
            RegionOutcome::Replace(RegionPayload::Tree(tree)) => RegionHighlight {
                map: result.map,
                source: tree.source().to_string(),
                tree: Some(tree),
                spans: Vec::new(),
            },
            RegionOutcome::Replace(RegionPayload::Spans { source, spans }) => RegionHighlight {
                map: result.map,
                source,
                tree: None,
                spans,
            },
            RegionOutcome::CarryForward { source } => {
                let (tree, spans) = carried
                    .get_mut(i)
                    .filter(|c| c.source == source)
                    .map(|c| (c.tree.take(), std::mem::take(&mut c.spans)))
                    .unwrap_or_default();
                RegionHighlight {
                    map: result.map,
                    source,
                    tree,
                    spans,
                }
            }
        })
        .collect();
    doc.highlight.truncated = reply.truncated;
    doc.highlight.version = version;
}

// `version` must already be checked against the live buffer by the caller —
// a reply describing content the buffer has moved past is dropped whole
// here, never partially applied.
pub(crate) fn apply_reply(doc: &mut Document, version: u64, reply: HighlightReply) {
    install_regions(doc, version, reply);
}

// A `Cmd` is dispatched even when every region's tree is already valid and
// there is nothing to parse — the regions still need their refreshed maps,
// and only a completed reply may write them; this function deliberately
// never installs a map directly.
pub(crate) fn schedule_highlight(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let version = doc.buffer.version();
    if doc.highlight.in_flight.is_some() {
        if let Some(doc) = app.doc_mut(id) {
            doc.highlight.pending = true;
        }
        return;
    }
    if doc.highlight.version == version {
        return;
    }
    let sources = resolve_region_sources(app, id);
    let Some(doc) = app.doc_mut(id) else { return };
    let jobs = plan_jobs(doc, sources);
    // Nothing to say: no code region and nothing stored either, so no
    // explicit "is this a code document" check is needed here.
    if jobs.is_empty() && doc.highlight.regions.is_empty() {
        return;
    }
    doc.highlight.in_flight = Some(version);
    effects.cmds.push(runtime::highlight_cmd(id, version, jobs));
}

fn plan_jobs(doc: &Document, sources: Vec<RegionSource>) -> Vec<RegionJob> {
    sources
        .into_iter()
        .enumerate()
        .map(|(i, source)| {
            let valid = doc
                .highlight
                .regions
                .get(i)
                .and_then(|region| region.tree.as_ref())
                .is_some_and(|tree| tree.source() == source.text);
            let work = if valid {
                RegionWork::Retain {
                    source: source.text,
                }
            } else {
                RegionWork::Parse {
                    lang: source.lang,
                    source: source.text,
                }
            };
            RegionJob {
                map: source.map,
                work,
            }
        })
        .collect()
}

// The one sanctioned synchronous parse on the main thread: bounded by
// `FIRST_PAINT_BUDGET` and run exactly once, strictly before the first
// draw — nothing is on screen yet, so even a full-budget miss blocks
// nothing a user can see.
//
// `FIRST_PAINT_BUDGET` is this pass's per-region cap AND its total, so the
// ceiling holds however many regions the document has: a region needing
// longer than the whole pre-draw ceiling cannot be afforded at any share of
// it. Missing a region costs nothing visible — the background pass follows
// immediately at a full `PARSE_BUDGET` per region.
pub(crate) fn first_paint_highlight(app: &mut App) {
    let id = app.active;
    let Some(doc) = app.doc(id) else { return };
    let version = doc.buffer.version();
    if doc.highlight.version == version {
        return;
    }
    let jobs: Vec<RegionJob> = resolve_region_sources(app, id)
        .into_iter()
        .map(|source| RegionJob {
            map: source.map,
            work: RegionWork::Parse {
                lang: source.lang,
                source: source.text,
            },
        })
        .collect();
    if jobs.is_empty() {
        return;
    }
    let budget = runtime::PassBudget::new(runtime::FIRST_PAINT_BUDGET, runtime::FIRST_PAINT_BUDGET);
    let PassOutcome::Replace(reply) = runtime::run_regions(jobs, budget) else {
        return;
    };
    if let Some(doc) = app.doc_mut(id) {
        install_regions(doc, version, reply);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;
