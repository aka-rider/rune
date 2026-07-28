//! Scheduling for the background tree-sitter highlight pass: decides when a
//! document's stored spans no longer describe its buffer and dispatches the
//! `Cmd` that recomputes them. Kept apart from the message dispatch so the
//! "at most one in flight per document" rule has one owner.

use crate::app::App;
use crate::document::DocumentId;
use crate::runtime::{self, Effects};

/// Requests a background highlight for `id` if its stored spans no longer
/// describe its buffer (plan WP5.S3) — the sole `Cmd`-dispatching entry
/// point for `rune_ts::highlight` (`Document::sync`/`App::sync_view` have
/// no `&mut Effects`). A no-op for a document with no highlightable
/// language. At most one highlight `Cmd` runs per document at a time — a
/// second call while one is in flight only arms `pending`, consumed by
/// `dispatch::handle_highlighted` once the reply lands.
pub(crate) fn schedule_highlight(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let Some(lang) = doc.kind.language() else {
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
    let cmd = runtime::highlight_cmd(id, version, lang, doc.buffer.content().to_string());
    effects.cmds.push(cmd);
}
