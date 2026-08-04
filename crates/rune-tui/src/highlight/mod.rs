//! Scheduling and state for the background tree-sitter highlight pass.
//!
//! There is ONE pipeline. Every region of code — a whole `.ts` file, a
//! ```` ```ts ```` fence inside a markdown document, an indented code block —
//! is the same thing: a `rune_md::element::code_region::CodeRegion` whose
//! source text is reconstructed prefix-free, parsed by `rune_ts::parse`, and
//! whose retained tree is queried per frame over the visible byte range only.
//! Identical code therefore renders identically wherever it sits, which is
//! the entire reason this module is shaped this way.
//!
//! Two pipelines used to exist. A whole code document retained a tree and got
//! a 5-second parse budget; a fence dropped its tree, queried its whole self
//! once, and got a 250ms budget DIVIDED by the number of fences in the
//! document — so four fences gave each 62ms and a large one rendered flat.
//! They also disagreed on clamping, on truncation reporting, and on whether a
//! timeout was surfaced at all. Collapsing them removed every one of those
//! divergences.
//!
//! One full budget per region is affordable because a pass is bounded twice:
//! the total it may spend is a constant of its own, and retained trees keep
//! the regions that actually need parsing few. A tree is still valid when
//! `tree.source() == map.reconstruct(content)`, so an edit inside one fence
//! reparses that fence alone and an edit in prose reparses nothing. The maps
//! still refresh on every version change — a region's BUFFER offsets move
//! when text above it changes even though its own text did not.
//!
//! Tree-sitter is never driven incrementally: `Tree::edit` risks a grammar
//! `ts_assert` `SIGABRT` and every parse here is a full parse of a region
//! (§1.3).

pub mod query;

use std::ops::Range;

use rune_syntax::ScopeId;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::linemap::LineMap;
use crate::runtime::{self, Effects};

pub use query::visible_spans;

/// The async highlight state for one document: one `RegionHighlight` per
/// code region, in document order, plus the bookkeeping shared by all of
/// them.
///
/// `version` is the buffer version `regions` describes — both their maps and
/// whatever their channels currently hold. `in_flight` carries the version a
/// currently-running highlight `Cmd` was spawned against; at most one may be
/// in flight per document (`spawn_cmd` has no thread pool and no
/// cancellation). `pending` records that a further edit landed while that
/// `Cmd` was still running, so its completion re-schedules rather than the
/// document going stale until the next keystroke.
///
/// A completion carrying `result: None` (every attempted region overran its
/// budget, or none resolved) leaves every region untouched: a slow document
/// degrades to STALE colours, never to NO colours.
#[derive(Debug, Default)]
pub struct HighlightState {
    pub version: u64,
    pub regions: Vec<RegionHighlight>,
    pub in_flight: Option<u64>,
    pub pending: bool,
    /// A producer hit its span cap and part of this document is uncoloured.
    /// Read back after storing a reply to drive a status line, unless that
    /// same reply also timed out (timeout wins and is shown instead).
    pub truncated: bool,
    /// Test-only instrumentation: counts how many times
    /// `resolve_region_sources` actually rebuilt this document's region
    /// sources — the full-text reconstruction the in-flight/version gates in
    /// `schedule_highlight` must skip whenever a highlight is already
    /// running. Per-`Document`, not a shared global, so parallel tests never
    /// interfere with each other's count.
    #[cfg(test)]
    pub resolve_calls: std::cell::Cell<usize>,
}

/// One code region's highlight state: where its bytes live, and whichever of
/// the two channels currently colours it.
///
/// `map` translates between this region's prefix-free reconstructed source
/// and real buffer offsets, in both directions.
///
/// `tree` is the ordinary channel — a retained parse the render path queries
/// per frame over the visible range only.
///
/// `spans` is the residual channel, in BUFFER coordinates, for the two cases
/// that cannot produce a tree: a ```` ```markdown ```` fence has no
/// tree-sitter grammar at all (markdown stays comrak's, so
/// `runtime::md_fence::markdown_fence_spans` emits spans directly), and the
/// session fuzzer's hostile span injection has no way to synthesize a
/// `ParsedTree`. A reply populates exactly one channel and clears the other;
/// the render path reads both, so neither can be silently ignored.
#[derive(Debug, Default)]
pub struct RegionHighlight {
    pub map: LineMap,
    pub tree: Option<rune_ts::ParsedTree>,
    pub spans: Vec<(Range<usize>, ScopeId)>,
}

/// What a highlight call produced for one region.
#[derive(Debug)]
pub enum RegionPayload {
    Tree(rune_ts::ParsedTree),
    /// Buffer-coordinate spans — already mapped back through the region's
    /// own `LineMap`, so a nested fence's container prefix is excluded by
    /// construction.
    Spans(Vec<(Range<usize>, ScopeId)>),
}

/// One region's slot in a reply: its refreshed map, plus the new payload
/// when the call produced one.
///
/// `payload: None` means "this region's channels carry forward unchanged" —
/// either its retained tree was still valid and no parse was attempted, or
/// the attempt overran its budget. Both cases want the same thing: keep the
/// colours, take the new map.
#[derive(Debug)]
pub struct RegionResult {
    pub map: LineMap,
    pub payload: Option<RegionPayload>,
}

/// A completed highlight call: one entry per code region, in document order,
/// describing the document's WHOLE region layout at the version the call ran
/// against. Applying it is a total replacement, never a patch — which is why
/// no index in it can ever refer to a region that does not exist.
#[derive(Debug)]
pub struct HighlightReply {
    pub regions: Vec<RegionResult>,
    pub truncated: bool,
}

/// One region's off-thread work item.
///
/// `work: None` means the scheduler already holds a valid tree for this
/// region: the job carries only the refreshed map, and the region is never
/// reparsed. That is the mechanism that keeps the regions competing for a
/// pass's total few enough for one full `PARSE_BUDGET` each to stay
/// affordable.
pub(crate) struct RegionJob {
    pub(crate) map: LineMap,
    pub(crate) work: Option<(RegionLang, String)>,
}

/// Which highlighter a region's info string resolves to: a tree-sitter
/// grammar name (`rune_ts::lang::resolve`'s own output), or the markdown
/// reveal-emit reuse path for a ```` ```markdown ````/```` ```md ```` fence.
/// `rune_ts::lang::resolve` never registers either markdown spelling, so the
/// two resolutions are mutually exclusive and `region_language` never has to
/// pick between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RegionLang {
    Ts(&'static str),
    Markdown,
}

/// One region as the scheduler resolved it, before deciding whether it needs
/// a parse: the highlighter, the coordinate map, and the prefix-free source
/// text both of those describe.
struct RegionSource {
    lang: RegionLang,
    map: LineMap,
    text: String,
}

/// Resolves a region's info string to a `RegionLang`: the first token after
/// splitting on whitespace AND `,` (a fence may be tagged
/// ```` ```rust,ignore ```` or ```` ```rust title=x ````).
/// `markdown`/`md` resolve to the comrak reveal-emit reuse path; every other
/// token is looked up through the compile-free `rune_ts::lang::resolve` —
/// safe on the UI thread, never the query-compiling registry getter. A tag
/// that doesn't resolve (an unknown language, or no tag at all) contributes
/// nothing and is not an error: `code_regions` deliberately emits such a
/// region because a consumer painting a background still cares about it, and
/// highlighting is simply the consumer with nothing to do for it.
fn region_language(info: &str) -> Option<RegionLang> {
    let token = info
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|s| !s.is_empty())?;
    if token.eq_ignore_ascii_case("markdown") || token.eq_ignore_ascii_case("md") {
        return Some(RegionLang::Markdown);
    }
    rune_ts::lang::resolve(token).map(RegionLang::Ts)
}

/// Every code region this document has a highlighter for, each carrying its
/// own `LineMap` and the PREFIX-FREE source text that map reconstructs.
///
/// `CodeRegion::content` is one `Range` per physical content line, and that
/// is what makes the reconstruction correct: for a fence nested inside a
/// blockquote or list item, the gap between two consecutive lines' buffer
/// ranges holds that container's own repeating prefix (`"> "`, a list
/// marker's indent), which must never reach a parser as source bytes —
/// tree-sitter's error recovery silently absorbs a stray `"> "` for some
/// grammars but an indentation-sensitive one loses most of its structure to
/// it.
///
/// A region with any line that somehow doesn't land on a live byte range of
/// the current buffer (should not happen — the ranges are derived from the
/// buffer's own parse — but `LineMap::reconstruct` degrades to "skip the
/// whole region" rather than a panic, per §1.3) is silently skipped.
fn region_sources(doc: &Document) -> Vec<RegionSource> {
    let content = doc.buffer.content();
    doc.doc
        .code_regions(&doc.buffer)
        .into_iter()
        .filter_map(|region| {
            let lang = region_language(&region.info)?;
            let map = LineMap::new(region.content);
            let text = map.reconstruct(content)?;
            Some(RegionSource { lang, map, text })
        })
        .collect()
}

/// Rebuilds the block tree, then resolves `id`'s region sources.
///
/// The rebuild has to happen before any region range is read: the settle
/// step that normally rebuilds it runs AFTER the update loop returns, so
/// without this the regions describe the PREVIOUS buffer version while the
/// command is stamped with the current one — a reply the version check would
/// then accept as authoritative, painting every region at a shifted offset
/// until the next keystroke. It costs nothing beyond this call:
/// `DocMachine::sync_content`'s own version guard makes it a no-op on every
/// call after the first per buffer version, so the settle step's own call
/// becomes the no-op instead of this one.
fn resolve_region_sources(app: &mut App, id: DocumentId) -> Vec<RegionSource> {
    if let Some(doc) = app.doc_mut(id) {
        doc.doc.sync_content(&doc.buffer);
        #[cfg(test)]
        doc.highlight
            .resolve_calls
            .set(doc.highlight.resolve_calls.get() + 1);
    }
    app.doc(id).map(region_sources).unwrap_or_default()
}

/// The ONE writer of `HighlightState::regions`.
///
/// A reply describes the document's whole region layout at `version`, so
/// installing it replaces the list outright. A slot with no payload inherits
/// the channels of whatever sat at the same position before — that is how a
/// region whose tree was still valid, and a region whose reparse overran its
/// budget, both keep their colours while taking their new map.
fn install_regions(doc: &mut Document, version: u64, reply: HighlightReply) {
    let mut carried = std::mem::take(&mut doc.highlight.regions);
    doc.highlight.regions = reply
        .regions
        .into_iter()
        .enumerate()
        .map(|(i, result)| match result.payload {
            Some(RegionPayload::Tree(tree)) => RegionHighlight {
                map: result.map,
                tree: Some(tree),
                spans: Vec::new(),
            },
            Some(RegionPayload::Spans(spans)) => RegionHighlight {
                map: result.map,
                tree: None,
                spans,
            },
            None => {
                let (tree, spans) = carried
                    .get_mut(i)
                    .map(|c| (c.tree.take(), std::mem::take(&mut c.spans)))
                    .unwrap_or_default();
                RegionHighlight {
                    map: result.map,
                    tree,
                    spans,
                }
            }
        })
        .collect();
    doc.highlight.truncated = reply.truncated;
    doc.highlight.version = version;
}

/// Applies a completed `Msg::Highlighted` reply. Lives here rather than in
/// `dispatch` so `install_regions` stays this module's private business.
///
/// `version` must already have been checked against the live buffer by the
/// caller: a reply describing content the buffer has moved past is dropped
/// whole, never partially applied.
pub(crate) fn apply_reply(doc: &mut Document, version: u64, reply: HighlightReply) {
    install_regions(doc, version, reply);
}

/// Requests a background highlight for `id` if its stored regions no longer
/// describe its buffer — the sole `Cmd`-dispatching entry point for a
/// background `rune_ts::parse` call (`Document::sync`/`App::sync_view` have
/// no `&mut Effects`).
///
/// At most one highlight `Cmd` runs per document at a time: a second call
/// while one is in flight only arms `pending`, consumed by
/// `dispatch::handle_highlighted` once the reply lands.
///
/// The in-flight/version gates run FIRST, before any region source is
/// resolved: resolving reconstructs every region's source text, and this fn
/// is called on every version-changing message — paying that only to discard
/// it because a highlight is already in flight (the overwhelmingly common
/// case while typing) was the cost this ordering removes.
///
/// A `Cmd` is dispatched even when every region's retained tree is still
/// valid and there is nothing to parse — the case an edit in prose between
/// two fences takes. The regions still need their refreshed maps, and this
/// function deliberately does not install them itself: scheduling runs from
/// inside the update loop, including from `handle_highlighted` when a
/// `pending` edit is consumed, so installing here would let a
/// `Msg::Highlighted` step change what renders without having adopted any
/// reply. Routing every region write through the reply keeps that state
/// unreachable rather than merely unlikely; the round trip costs a thread
/// hop and still parses nothing.
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
    // A document with no code region and no stored one has nothing to say:
    // the reply would carry an empty layout over an already-empty one. This
    // is the whole reason an image document, or any prose-only markdown
    // document, never dispatches a highlight `Cmd` — no kind check exists or
    // is needed, because `code_regions` already answered the question.
    if jobs.is_empty() && doc.highlight.regions.is_empty() {
        return;
    }
    doc.highlight.in_flight = Some(version);
    effects.cmds.push(runtime::highlight_cmd(id, version, jobs));
}

/// Turns resolved sources into one job per region, marking each as needing a
/// parse or not.
///
/// A region's retained tree is valid exactly when it was parsed from the
/// same bytes the region reconstructs to now. Region identity across an edit
/// is positional — the same index in document order — which is stable for
/// every edit that doesn't add or remove a region, and is the same identity
/// `install_regions` inherits channels by.
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
            if valid {
                RegionJob {
                    map: source.map,
                    work: None,
                }
            } else {
                RegionJob {
                    map: source.map,
                    work: Some((source.lang, source.text)),
                }
            }
        })
        .collect()
}

/// The one sanctioned synchronous parse on the main thread — bounded by
/// `runtime::FIRST_PAINT_BUDGET` and made exactly once, from `runtime::run`'s
/// bootstrap, strictly before the first draw: nothing is on screen yet, so
/// even a full-budget miss blocks nothing a user can see. CONSTITUTION §5.3
/// ("`Update()`/`Init()` stay non-blocking") is about `app::update`, which
/// this deliberately never calls into and is never called from.
///
/// Applies to regions generally, not only to code documents: a markdown
/// document whose first screen is a fence gets frame 1 already coloured for
/// exactly the same reason a `.ts` file does.
///
/// On success the regions are installed and `version` stamped — exactly what
/// a completed background reply would do — so `schedule_highlight`'s own
/// already-current guard makes the runtime's bootstrap kick a no-op for this
/// document; a failed or skipped attempt leaves `version` untouched, so that
/// same kick still dispatches the ordinary background `Cmd`.
///
/// `FIRST_PAINT_BUDGET` is this pass's per-region cap AND its total, so the
/// ceiling holds however many regions the document turns out to have: a
/// region that would need longer than the whole pre-draw ceiling cannot be
/// afforded at any share of it, and the fast regions ahead of it are exactly
/// the ones worth colouring on frame 1. Missing a region costs nothing
/// visible — the background pass follows immediately at a full
/// `PARSE_BUDGET` per region.
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
            work: Some((source.lang, source.text)),
        })
        .collect();
    if jobs.is_empty() {
        return;
    }
    let budget = runtime::PassBudget::new(runtime::FIRST_PAINT_BUDGET, runtime::FIRST_PAINT_BUDGET);
    let Some(reply) = runtime::run_regions(jobs, budget) else {
        return;
    };
    if let Some(doc) = app.doc_mut(id) {
        install_regions(doc, version, reply);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
