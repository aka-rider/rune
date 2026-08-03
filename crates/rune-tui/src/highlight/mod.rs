//! Scheduling for the background tree-sitter highlight pass: decides when a
//! document's stored tree/spans no longer describe its buffer and dispatches
//! the `Cmd` that recomputes them. Kept apart from the message dispatch so
//! the "at most one in flight per document" rule has one owner. Plan WP6
//! adds this module's second source: a `Markdown` document's own fenced code
//! blocks. Both sources flow into the SAME `Msg::Highlighted` and the SAME
//! `HighlightState` — there is no second message and no second overlay
//! (plan WP6, "reuse the existing message and state"). A whole code
//! document's parse is a single bounded attempt (D5) with no retry chain: a
//! `None` reply is surfaced via `dispatch::handle_highlighted`'s status
//! message instead of being retried at a widened budget.

use std::ops::Range;

use rune_syntax::{DocumentKind, ScopeId};

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::linemap::LineMap;
use crate::runtime::{self, Effects};

/// The async highlight state for one document (plan WP5, extended by the
/// syntax-highlighting-latency plan's WP3): the last spans a background
/// highlight call actually delivered, tagged with the buffer `version` they
/// describe. `in_flight` carries the version a currently-running highlight
/// `Cmd` was spawned against — at most one may be in flight per document
/// (`spawn_cmd` has no thread pool or cancellation); `pending` records that a
/// further edit landed while that `Cmd` was still running, so its completion
/// re-schedules instead of the document going stale until the next
/// keystroke. A completion carrying `result: None` (budget elapsed, unknown
/// language, parse failure) leaves both `tree` and `spans` untouched — see
/// `Msg::Highlighted`'s doc comment: a slow document degrades to STALE
/// colours, never to NO colours.
///
/// `tree` (D6) is the retained whole-document parse a code document's
/// background `Cmd` delivers — the render path queries it per frame,
/// restricted to the visible byte range, rather than replaying a
/// whole-document span list. `spans` stays alongside it: a markdown
/// document's fences never populate `tree` (D6 keeps the fence pipeline on
/// the span path), and the session fuzzer's hostile span injection has no
/// way to synthesize a `ParsedTree` either. Both fields share the same
/// `version`/`in_flight`/`pending` discipline — whichever payload a reply
/// carries, the bookkeeping is identical.
#[derive(Debug, Default)]
pub struct HighlightState {
    pub version: u64,
    pub spans: Vec<(Range<usize>, ScopeId)>,
    pub tree: Option<rune_ts::ParsedTree>,
    pub in_flight: Option<u64>,
    pub pending: bool,
    /// The producer hit its span cap and the tail of this document is
    /// uncoloured. Read back after storing a `Spans` reply to drive a
    /// status line telling the user the tail is uncoloured, unless that
    /// same reply also timed out (timeout wins and is shown instead).
    pub truncated: bool,
    /// Test-only instrumentation (plan WP16.S2): counts how many times
    /// `highlight::resolve_highlight_source` actually built a
    /// `HighlightSource` for this document — the full-buffer clone the
    /// in-flight/version gates in `schedule_highlight` must skip whenever a
    /// highlight is already running. Per-`Document`, not a shared global, so
    /// parallel tests never interfere with each other's count.
    #[cfg(test)]
    pub resolve_calls: std::cell::Cell<usize>,
}

/// What `schedule_highlight` found to highlight this call (plan WP6.S3) —
/// a whole-buffer language (`DocumentKind::Code`) or a markdown document's
/// own resolvable fences. Neither variant borrows the `Document` it was
/// derived from: `code_fence_sources` copies out both the language name
/// (already `&'static str` — `rune_ts::lang::resolve`'s own output) and
/// each fence's reconstructed source text before returning, so a
/// `HighlightSource` can outlive the `&Document` borrow that produced it
/// and survive past the `app.doc_mut(id)` call below.
enum HighlightSource {
    Whole(&'static str, String),
    Fences(Vec<(FenceLang, LineMap, String)>),
}

/// Which highlighter a fence's info string resolves to (plan WP6.S2): a
/// tree-sitter grammar name (`rune_ts::lang::resolve`'s own output), or the
/// markdown reveal-emit reuse path for a ```` ```markdown ````/```` ```md ````
/// fence — `rune_ts::lang::resolve` never registers either spelling
/// ("markdown stays comrak's", `rune_ts::lang`'s own doc comment), so the two
/// resolutions are mutually exclusive and `fence_language` never has to pick
/// between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FenceLang {
    Ts(&'static str),
    Markdown,
}

/// What `schedule_highlight` resolves before it can decide what to
/// dispatch: `None` when `id` has no highlightable language and no
/// resolvable fence, exactly `schedule_highlight`'s old inline early-return
/// conditions. Rebuilds the block tree first (see `schedule_highlight`'s own
/// doc comment for why) — a no-op via `DocMachine::sync_content`'s own
/// version guard on every call after the first per buffer version.
fn resolve_highlight_source(app: &mut App, id: DocumentId) -> Option<HighlightSource> {
    if let Some(doc) = app.doc_mut(id) {
        doc.doc.sync_content(&doc.buffer);
        #[cfg(test)]
        doc.highlight
            .resolve_calls
            .set(doc.highlight.resolve_calls.get() + 1);
    }
    let doc = app.doc(id)?;
    // Plan WP4.S9: `schedule_highlight` itself has no `db`/`file_path`/
    // `read_only` guard — only `in_flight` and `version` — so this is the
    // one place an image document is excluded from ever dispatching a
    // highlight `Cmd`. `doc.kind.language()` already returns `None` for
    // `Image` and `is_markdown()` is `false`, so the `else` arm below would
    // reach the same `None` regardless; this explicit early return states
    // the invariant directly rather than leaving it as an incidental
    // consequence of two unrelated checks.
    if doc.kind == DocumentKind::Image {
        return None;
    }
    if let Some(lang) = doc.kind.language() {
        Some(HighlightSource::Whole(
            lang,
            doc.buffer.content().to_string(),
        ))
    } else if doc.kind.is_markdown() {
        let fences = code_fence_sources(doc);
        if fences.is_empty() {
            None
        } else {
            Some(HighlightSource::Fences(fences))
        }
    } else {
        None
    }
}

/// The chokepoint `schedule_highlight` uses to turn a resolved
/// `HighlightSource` into the right `Cmd` — a whole document parses through
/// `highlight_cmd` (the retained-tree path, `PARSE_BUDGET`); a markdown
/// document's fences parse through `fence_highlight_cmd` (the span path,
/// `HIGHLIGHT_BUDGET`).
fn dispatch_highlight_cmd(id: DocumentId, version: u64, source: HighlightSource) -> runtime::Cmd {
    match source {
        HighlightSource::Whole(lang, text) => runtime::highlight_cmd(id, version, lang, text),
        HighlightSource::Fences(fences) => runtime::fence_highlight_cmd(id, version, fences),
    }
}

/// Requests a background highlight for `id` if its stored tree/spans no
/// longer describe its buffer (plan WP5.S3) — the sole `Cmd`-dispatching
/// entry point for a background `rune_ts::parse`/`highlight` call
/// (`Document::sync`/`App::sync_view` have no `&mut Effects`). A no-op for a
/// document with no highlightable language and no resolvable fence. At most
/// one highlight `Cmd` runs per document at a time — a second call while one
/// is in flight only arms `pending`, consumed by `dispatch::
/// handle_highlighted` once the reply lands. Also the guard that makes the
/// startup bootstrap kick a no-op once `highlight::first_paint_highlight`
/// already populated a document's tree synchronously: `highlight.version ==
/// version` is set by that success path exactly like any other completed
/// highlight, so the early return below fires for it identically.
///
/// The in-flight/version gates run FIRST, before `resolve_highlight_source`
/// (plan WP16.S2): `HighlightSource::Whole` clones the entire buffer to a
/// `String` to cross the `Cmd` thread boundary, and this fn is called on
/// every version-changing message — cloning a large buffer only to then
/// discard it because a highlight is already in flight (the overwhelmingly
/// common case while typing) was the cost this reorder removes. The clone
/// now happens only on the call that actually dispatches a `Cmd`. This
/// leaves the fence block-tree rebuild that `resolve_highlight_source`
/// performs (see its own doc comment) as the one thing skipped whenever the
/// gates below short-circuit — harmless, since the settle step that
/// normally rebuilds it runs again after the update loop returns, and the
/// gates below never accept a stale fence range as authoritative: a version
/// that has already been highlighted, or a highlight already in flight,
/// means no new `Cmd` is dispatched against whatever the block tree
/// currently says regardless.
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
    // Rebuild the block tree before reading fence ranges. The settle step
    // that normally does this runs AFTER the update loop returns, so without
    // this the fences describe the PREVIOUS buffer version while the command
    // is stamped with the current one — a reply the version check would then
    // accept as authoritative, painting every fence at a shifted offset until
    // the next edit happens to schedule again. Costs nothing beyond this
    // call: `DocMachine::sync_content`'s own version guard makes it a no-op
    // on every call after the first per buffer version, so the settle step's
    // own call becomes the no-op instead of this one. (`resolve_highlight_
    // source` performs the actual `sync_content` call, and — now that the
    // gates above have already run — is only reached on the call that will
    // actually dispatch a `Cmd`.)
    let Some(source) = resolve_highlight_source(app, id) else {
        return;
    };
    let Some(doc) = app.doc_mut(id) else { return };
    doc.highlight.in_flight = Some(version);
    effects
        .cmds
        .push(dispatch_highlight_cmd(id, version, source));
}

/// Resolves a fenced code block's info string to a `FenceLang` (plan
/// WP6.S2): the first token after splitting on whitespace AND `,` (a fence
/// may be tagged ```` ```rust,ignore ```` or ```` ```rust title=x ````).
/// `markdown`/`md` resolve to `FenceLang::Markdown` (the comrak reveal-emit
/// reuse path); every other token is looked up through the compile-free
/// `rune_ts::lang::resolve` — safe here on the UI thread `[B5]`, never the
/// query-compiling registry getter, and which never registers either
/// markdown spelling itself ("markdown stays comrak's"). A tag that doesn't
/// resolve (an unknown language, or no tag at all) contributes nothing and
/// is not an error.
fn fence_language(info: &str) -> Option<FenceLang> {
    let token = info
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|s| !s.is_empty())?;
    if token.eq_ignore_ascii_case("markdown") || token.eq_ignore_ascii_case("md") {
        return Some(FenceLang::Markdown);
    }
    rune_ts::lang::resolve(token).map(FenceLang::Ts)
}

/// Narrows a markdown document's code regions to the ones THIS consumer can
/// act on — those whose info string resolves to a known highlighter — each
/// carrying its own per-line buffer ranges and a reconstructed, PREFIX-FREE
/// owned source text, so `fence_highlight_cmd` can move the result across the
/// `Cmd` thread boundary exactly like `highlight_cmd` moves a whole code
/// document's content.
///
/// A region with an empty or unresolvable info string is dropped HERE, not
/// upstream: `code_regions` deliberately emits it (a region is a region
/// whether or not a grammar exists for it), and highlighting is simply the
/// one consumer that has nothing to do with it.
///
/// `CodeRegion::content` is one `Range` per physical content line, and that
/// is what makes the reconstruction correct: for a fence nested inside a
/// blockquote or list item, the gap between two consecutive lines' buffer
/// ranges holds that container's own repeating prefix (`"> "`, a list
/// marker's indent), which must never reach `rune_ts::highlight` as source
/// bytes — tree-sitter's error recovery silently absorbs a stray `"> "` for
/// some grammars (Rust) but not others (an indentation-sensitive grammar
/// like YAML loses most of its structure to it). The `LineMap` those ranges
/// build both reconstructs the PREFIX-FREE source text and maps spans
/// parsed against that text back to real buffer offsets.
///
/// A region with any line that somehow doesn't land on a live byte range of
/// the current buffer (should not happen — the ranges are derived from the
/// buffer's own parse — but `LineMap::reconstruct` degrades to "skip the
/// whole region" rather than a panic, per §1.3) is silently skipped.
fn code_fence_sources(doc: &Document) -> Vec<(FenceLang, LineMap, String)> {
    let content = doc.buffer.content();
    doc.doc
        .code_regions(&doc.buffer)
        .into_iter()
        .filter_map(|region| {
            let lang = fence_language(&region.info)?;
            let map = LineMap::new(region.content);
            let text = map.reconstruct(content)?;
            Some((lang, map, text))
        })
        .collect()
}

/// The one sanctioned synchronous `rune_ts::parse` call on the main thread
/// (D4 of the syntax-highlighting-latency plan) — bounded by
/// `runtime::FIRST_PAINT_BUDGET` and made exactly once, from `runtime::run`'s
/// bootstrap, strictly before the first draw: nothing is on screen yet, so
/// even a full-budget miss blocks nothing a user can see. CONSTITUTION §5.3
/// ("`Update()`/`Init()` stay non-blocking") is about `app::update`, which
/// this deliberately never calls into and is never called from — the ONE
/// caller is `runtime::run` itself, before its own event loop starts.
///
/// A no-op unless the startup document is a CODE document (`doc.kind.
/// language()` resolves) with no tree yet (`doc.highlight.tree.is_none()`
/// — an idempotent guard, so calling this twice, or after some other path
/// already populated the tree, costs nothing). On a successful parse, the
/// tree is stored and `doc.highlight.version` is stamped to the buffer's
/// current version — exactly what a completed background `Msg::Highlighted`
/// reply would do — so `schedule_highlight`'s own `version == version`
/// early-return makes the runtime's bootstrap kick a no-op for this
/// document; a failed or skipped attempt leaves `version` untouched, so that
/// same kick still dispatches the ordinary background `Cmd` exactly as
/// before this function existed.
pub(crate) fn first_paint_highlight(app: &mut App) {
    let id = app.active;
    let Some(doc) = app.doc(id) else { return };
    if doc.highlight.tree.is_some() {
        return;
    }
    let Some(lang) = doc.kind.language() else {
        return;
    };
    let source = doc.buffer.content().to_string();
    let version = doc.buffer.version();

    let Some(tree) = rune_ts::parse(lang, &source, runtime::FIRST_PAINT_BUDGET) else {
        return;
    };

    if let Some(doc) = app.doc_mut(id) {
        doc.highlight.tree = Some(tree);
        doc.highlight.version = version;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;

    use super::*;
    use crate::app::App;

    fn app_for(content: &str, path: &str) -> App {
        App::new(
            Buffer::new(content),
            Some(PathBuf::from(path)),
            Arc::new(Mem::new()),
            None,
        )
    }

    #[test]
    fn schedule_highlight_skips_the_source_clone_while_one_is_already_in_flight() {
        // The keystroke-latency regression WP16.S2 fixes: `schedule_highlight`
        // ran `resolve_highlight_source` (which clones the ENTIRE buffer to a
        // `String` for a whole-language document) before checking whether a
        // highlight was already in flight — so every version-changing message
        // paid that clone even on the overwhelmingly common path where the
        // gate immediately discards it. This asserts the gate now runs first.
        let content = "fn main() {}\n";
        let mut app = app_for(content, "/x/main.rs");
        let id = app.active;

        let version = app.doc(id).expect("doc").buffer.version();
        app.doc_mut(id).expect("doc").highlight.in_flight = Some(version);

        let mut effects = Effects::default();
        super::schedule_highlight(&mut app, id, &mut effects);

        let doc = app.doc(id).expect("doc");
        assert!(
            doc.highlight.pending,
            "a call while in_flight is set must arm pending"
        );
        assert!(
            effects.cmds.is_empty(),
            "a call while in_flight is set must not dispatch a second cmd"
        );
        assert_eq!(
            doc.highlight.resolve_calls.get(),
            0,
            "the full-buffer source clone must not run while a highlight is \
             already in flight — the in-flight gate must be checked BEFORE \
             resolve_highlight_source, not after"
        );
    }

    #[test]
    fn schedule_highlight_resolves_and_dispatches_when_no_highlight_is_in_flight() {
        // The converse of the case above: with no highlight running and no
        // stored version yet, the gates must fall through and the source
        // must actually be resolved once, dispatching exactly one cmd.
        let content = "fn main() {}\n";
        let mut app = app_for(content, "/x/main.rs");
        let id = app.active;

        let mut effects = Effects::default();
        super::schedule_highlight(&mut app, id, &mut effects);

        let doc = app.doc(id).expect("doc");
        assert_eq!(effects.cmds.len(), 1, "expected exactly one dispatched cmd");
        assert_eq!(
            doc.highlight.resolve_calls.get(),
            1,
            "resolve_highlight_source must run exactly once for the call that dispatches"
        );
    }

    /// D4's success path: a small `.rs` startup document parses inside
    /// `FIRST_PAINT_BUDGET` synchronously, populating `highlight.tree`
    /// before any `Cmd` ever runs — and, since `first_paint_highlight`
    /// stamps `highlight.version` on success exactly like a completed
    /// background reply would, a subsequent `schedule_highlight` call finds
    /// the document already current and pushes no `Cmd` at all (verifying
    /// the plan's "the already-current guard suppresses the bootstrap Cmd"
    /// claim rather than assuming it).
    #[test]
    fn first_paint_highlights_small_file_synchronously() {
        let mut app = App::new(
            Buffer::new("fn main() {}\n"),
            Some(PathBuf::from("/x/main.rs")),
            Arc::new(Mem::new()),
            None,
        );
        let id = app.active;

        first_paint_highlight(&mut app);

        let doc = app.doc(id).expect("doc");
        assert!(
            doc.highlight.tree.is_some(),
            "a trivial rust source must parse within the generous first-paint budget"
        );
        assert_eq!(doc.highlight.version, doc.buffer.version());

        let mut effects = Effects::default();
        schedule_highlight(&mut app, id, &mut effects);
        assert!(
            effects.cmds.is_empty(),
            "the already-current guard must suppress the bootstrap Cmd once \
             first_paint_highlight already populated this document's tree"
        );
    }

    /// A markdown (non-code) startup document has no language to parse —
    /// `first_paint_highlight` must be a clean no-op, never touching `tree`.
    #[test]
    fn first_paint_highlight_is_a_no_op_for_a_non_code_document() {
        let mut app = App::new(Buffer::new("# hello\n"), None, Arc::new(Mem::new()), None);
        let id = app.active;

        first_paint_highlight(&mut app);

        assert!(app.doc(id).expect("doc").highlight.tree.is_none());
    }
}
