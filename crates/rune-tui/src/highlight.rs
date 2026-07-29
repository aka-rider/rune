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

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::runtime::{self, Effects};

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
    Fences(Vec<(&'static str, Vec<Range<usize>>, String)>),
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
    }
    let doc = app.doc(id)?;
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
pub(crate) fn schedule_highlight(app: &mut App, id: DocumentId, effects: &mut Effects) {
    // Rebuild the block tree before reading fence ranges. The settle step
    // that normally does this runs AFTER the update loop returns, so without
    // this the fences describe the PREVIOUS buffer version while the command
    // is stamped with the current one — a reply the version check would then
    // accept as authoritative, painting every fence at a shifted offset until
    // the next edit happens to schedule again. Costs nothing: this is
    // version-guarded and early-returns, so the settle step's own call
    // becomes the no-op instead of this one. (`resolve_highlight_source`
    // performs the actual `sync_content` call.)
    let Some(source) = resolve_highlight_source(app, id) else {
        return;
    };
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
    let Some(doc) = app.doc_mut(id) else { return };
    doc.highlight.in_flight = Some(version);
    effects
        .cmds
        .push(dispatch_highlight_cmd(id, version, source));
}

/// Resolves a fenced code block's info string to a canonical language name
/// (plan WP6.S2): the first token after splitting on whitespace AND `,` (a
/// fence may be tagged ```` ```rust,ignore ```` or ```` ```rust title=x ````),
/// looked up through the compile-free `rune_ts::lang::resolve` — safe here
/// on the UI thread `[B5]`, never the query-compiling registry getter. A
/// tag that doesn't resolve (an unknown language, or no tag at all)
/// contributes nothing and is not an error.
fn fence_language(info: &str) -> Option<&'static str> {
    let token = info
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|s| !s.is_empty())?;
    rune_ts::lang::resolve(token)
}

/// Collects every fence in a markdown document whose info string resolves
/// to a known language (plan WP6.S3), each carrying its own per-line buffer
/// ranges and a reconstructed, PREFIX-FREE owned source text — so `fence_
/// highlight_cmd` can move the result across the `Cmd` thread boundary
/// exactly like `highlight_cmd` moves a whole code document's content.
///
/// `code_fences` returns one `Range` per physical content line (finding A):
/// for a fence nested inside a blockquote or list item, the gap between two
/// consecutive lines' buffer ranges holds that container's own repeating
/// prefix (`"> "`, a list marker's indent), which must never reach
/// `rune_ts::highlight` as source bytes — tree-sitter's error recovery
/// silently absorbs a stray `"> "` for some grammars (Rust) but not others
/// (an indentation-sensitive grammar like YAML loses most of its structure
/// to it). `text` is built by joining each line's own slice with a single
/// `'\n'`, which reproduces the ORIGINAL buffer bytes exactly for a
/// top-level fence (the true gap between two top-level lines is exactly one
/// `'\n'` already) and drops every prefix byte for a nested one. `lines` is
/// carried alongside so `runtime::map_reconstructed_span` can map spans
/// parsed against this reconstructed text back to real buffer offsets.
///
/// A fence with any line that somehow doesn't land on a live byte range of
/// the current buffer (should not happen — `code_fences` derives its
/// ranges from the buffer's own parse — but `.get` degrades to "skip the
/// whole fence" rather than a panic, per §1.3) is silently skipped.
fn code_fence_sources(doc: &Document) -> Vec<(&'static str, Vec<Range<usize>>, String)> {
    let content = doc.buffer.content();
    doc.doc
        .code_fences()
        .into_iter()
        .filter_map(|(info, lines)| {
            let lang = fence_language(info)?;
            let pieces: Option<Vec<&str>> = lines.iter().map(|l| content.get(l.clone())).collect();
            let text = pieces?.join("\n");
            Some((lang, lines, text))
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
