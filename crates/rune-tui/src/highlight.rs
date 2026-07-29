//! Scheduling for the background tree-sitter highlight pass: decides when a
//! document's stored spans no longer describe its buffer and dispatches the
//! `Cmd` that recomputes them. Kept apart from the message dispatch so the
//! "at most one in flight per document" rule has one owner. Plan WP6 adds
//! this module's second source: a `Markdown` document's own fenced code
//! blocks. Both sources flow into the SAME `Msg::Highlighted` and the SAME
//! `HighlightState` — there is no second message and no second overlay
//! (plan WP6, "reuse the existing message and state"). `retry_highlight`
//! (finding B) is the one exception: its reply is the distinct `Msg::
//! HighlightRetried`, deliberately not `Msg::Highlighted` again — see that
//! variant's own doc comment for why.

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

/// `schedule_highlight` and `retry_highlight` (finding B) share every step
/// up to "what to dispatch and at what budget" — this resolves the former:
/// `None` when `id` has no highlightable language and no resolvable fence,
/// exactly `schedule_highlight`'s old inline early-return conditions.
/// Rebuilds the block tree first (see `schedule_highlight`'s own doc
/// comment for why) — a no-op via `DocMachine::sync_content`'s own version
/// guard on every call after the first per buffer version, so `retry_
/// highlight` calling this a second time against an unchanged buffer costs
/// nothing.
fn resolve_highlight_source(app: &mut App, id: DocumentId) -> Option<HighlightSource> {
    if let Some(doc) = app.doc_mut(id) {
        doc.doc.sync_content(&doc.buffer);
        #[cfg(test)]
        doc.highlight.resolve_calls.set(doc.highlight.resolve_calls.get() + 1);
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

/// The chokepoint `schedule_highlight` and `retry_highlight` both use to
/// turn a resolved `HighlightSource` into the right `Cmd` — `is_retry`
/// picks `runtime::highlight_retry_cmd`/`fence_highlight_retry_cmd` (the
/// widened budget, `Msg::HighlightRetried` reply) over the normal pair.
/// `reparser` (plan WP16.S3) is `id`'s own retained incremental-parse
/// state, shared into the `Whole` variant's `Cmd` — the `Fences` variant
/// ignores it, since each fence is reparsed fresh from its reconstructed
/// source every call regardless.
fn dispatch_highlight_cmd(
    id: DocumentId,
    version: u64,
    source: HighlightSource,
    is_retry: bool,
    reparser: std::sync::Arc<std::sync::Mutex<rune_ts::Reparser>>,
) -> runtime::Cmd {
    match (source, is_retry) {
        (HighlightSource::Whole(lang, text), false) => {
            runtime::highlight_cmd(id, version, lang, text, reparser)
        }
        (HighlightSource::Whole(lang, text), true) => {
            runtime::highlight_retry_cmd(id, version, lang, text, reparser)
        }
        (HighlightSource::Fences(fences), false) => {
            runtime::fence_highlight_cmd(id, version, fences)
        }
        (HighlightSource::Fences(fences), true) => {
            runtime::fence_highlight_retry_cmd(id, version, fences)
        }
    }
}

/// Requests a background highlight for `id` if its stored spans no longer
/// describe its buffer (plan WP5.S3) — the sole `Cmd`-dispatching entry
/// point for `rune_ts::highlight` (`Document::sync`/`App::sync_view` have
/// no `&mut Effects`). A no-op for a document with no highlightable
/// language and no resolvable fence. At most one highlight `Cmd` runs per
/// document at a time — a second call while one is in flight only arms
/// `pending`, consumed by `dispatch::handle_highlighted` once the reply
/// lands.
///
/// The in-flight/version gates run FIRST, before `resolve_highlight_source`
/// (plan WP16.S2): `HighlightSource::Whole` clones the entire buffer to a
/// `String` to cross the `Cmd` thread boundary, and this fn is called on
/// every version-changing message — cloning a large buffer only to then
/// discard it because a highlight is already in flight (the overwhelmingly
/// common case while typing) was the cost this reorder removes. The clone
/// now happens only on the call that actually dispatches a `Cmd`.
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
    let reparser = doc.highlight.reparser.clone();
    effects
        .cmds
        .push(dispatch_highlight_cmd(id, version, source, false, reparser));
}

/// Finding B's single bounded retry: called only from `dispatch::
/// handle_highlighted` when a `None` reply lands for a document that has
/// never had spans (`doc.highlight.version == 0`, `Buffer::version` never
/// being 0 itself). Reruns the SAME source at `HIGHLIGHT_RETRY_BUDGET`
/// through `Msg::HighlightRetried`, a reply `dispatch::handle_highlight_
/// retried` never re-arms — so this can only ever fire once per failed
/// first attempt, without a per-document attempt counter to keep in sync.
/// Re-checks that `version` still matches the live buffer before doing
/// anything (a defensive mirror of `schedule_highlight`'s own version
/// gate): if an edit landed in between, that edit's own `schedule_highlight`
/// call already owns this document's `in_flight`, and retrying the stale
/// version here would race it.
pub(crate) fn retry_highlight(app: &mut App, id: DocumentId, version: u64, effects: &mut Effects) {
    let Some(source) = resolve_highlight_source(app, id) else {
        return;
    };
    let Some(doc) = app.doc(id) else { return };
    if doc.buffer.version() != version {
        return;
    }
    let Some(doc) = app.doc_mut(id) else { return };
    doc.highlight.in_flight = Some(version);
    let reparser = doc.highlight.reparser.clone();
    effects
        .cmds
        .push(dispatch_highlight_cmd(id, version, source, true, reparser));
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;

    use crate::app::App;
    use crate::runtime::Effects;

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
}
