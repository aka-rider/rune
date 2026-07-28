//! Scheduling for the background tree-sitter highlight pass: decides when a
//! document's stored spans no longer describe its buffer and dispatches the
//! `Cmd` that recomputes them. Kept apart from the message dispatch so the
//! "at most one in flight per document" rule has one owner. Plan WP6 adds
//! this module's second source: a `Markdown` document's own fenced code
//! blocks. Both sources flow into the SAME `Msg::Highlighted` and the SAME
//! `HighlightState` — there is no second message and no second overlay
//! (plan WP6, "reuse the existing message and state").

use std::ops::Range;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::runtime::{self, Effects};

/// What `schedule_highlight` found to highlight this call (plan WP6.S3) —
/// a whole-buffer language (`DocumentKind::Code`) or a markdown document's
/// own resolvable fences. Neither variant borrows the `Document` it was
/// derived from: `code_fence_sources` copies out both the language name
/// (already `&'static str` — `rune_ts::lang::resolve`'s own output) and
/// each fence's source text before returning, so a `HighlightSource` can
/// outlive the `&Document` borrow that produced it and survive past the
/// `app.doc_mut(id)` call below.
enum HighlightSource {
    Whole(&'static str),
    Fences(Vec<(&'static str, Range<usize>, String)>),
}

/// Requests a background highlight for `id` if its stored spans no longer
/// describe its buffer (plan WP5.S3) — the sole `Cmd`-dispatching entry
/// point for `rune_ts::highlight` (`Document::sync`/`App::sync_view` have
/// no `&mut Effects`). A no-op for a document with no highlightable
/// language and no resolvable fence. At most one highlight `Cmd` runs per
/// document at a time — a second call while one is in flight only arms
/// `pending`, consumed by `dispatch::handle_highlighted` once the reply
/// lands.
pub(crate) fn schedule_highlight(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let source = if let Some(lang) = doc.kind.language() {
        HighlightSource::Whole(lang)
    } else if doc.kind.is_markdown() {
        let fences = code_fence_sources(doc);
        if fences.is_empty() {
            return;
        }
        HighlightSource::Fences(fences)
    } else {
        return;
    };
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
    let cmd = match source {
        HighlightSource::Whole(lang) => {
            runtime::highlight_cmd(id, version, lang, doc.buffer.content().to_string())
        }
        HighlightSource::Fences(fences) => runtime::fence_highlight_cmd(id, version, fences),
    };
    effects.cmds.push(cmd);
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
/// to a known language (plan WP6.S3), each carrying its own byte range and
/// owned source text — so `fence_highlight_cmd` can move the result across
/// the `Cmd` thread boundary exactly like `highlight_cmd` moves a whole
/// code document's content. A fence range that somehow doesn't land on a
/// live byte range of the current buffer (should not happen — `code_fences`
/// derives its ranges from the buffer's own parse — but `.get` degrades to
/// "skip" rather than a panic, per §1.3) is silently skipped.
fn code_fence_sources(doc: &Document) -> Vec<(&'static str, Range<usize>, String)> {
    let content = doc.buffer.content();
    doc.doc
        .code_fences()
        .into_iter()
        .filter_map(|(info, range)| {
            let lang = fence_language(info)?;
            let text = content.get(range.clone())?.to_string();
            Some((lang, range, text))
        })
        .collect()
}
