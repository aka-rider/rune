//! One shared way every integration test forces a document dirty without a
//! real edit (plan WP1: dirtiness is a content comparison against
//! `saved_content` now, so the old `saved_version = 0` trick is inert —
//! `saved_version` is no longer part of the comparison at all). Integration
//! test files are separate binaries, so this is the one place the fixture
//! lives rather than re-open-coding it per file (`app_quit_and_dispatch.rs`,
//! `save_flow.rs`, `db_wiring_degraded.rs`, `image_document.rs` all pull
//! this in via `mod dirty_common;`).
#![allow(dead_code)]

use std::sync::Arc;

use rune_tui::app::App;
use rune_tui::document::DocumentId;

/// Forces `id` dirty by moving its saved-content baseline away from the
/// live buffer — guaranteed to differ regardless of what the buffer holds
/// (including empty), since a NUL byte never occurs in a `Buffer`'s own
/// UTF-8 content. Also refreshes the render-only dirty cache through
/// `App::recompute_dirty` (CONSTITUTION §1.4.8: nothing reads `saved_content`
/// directly), so `doc.is_dirty()`/`app.is_dirty()` observe the change
/// immediately, exactly as they would after a real edit.
///
/// Also lowers `saved_version` one below the live buffer's version — a real
/// edit always moves both `saved_content` and `saved_version` out of sync
/// with the buffer TOGETHER, and a test that goes on to drive an actual
/// save through needs `Document::finish_save_ok`'s own `version > saved_
/// version` gate to pass, or the round trip would silently promote nothing
/// and leave the document dirty forever despite the save having "worked".
pub fn force_dirty(app: &mut App, id: DocumentId) {
    let Some(doc) = app.doc_mut(id) else { return };
    let marker: Arc<str> = Arc::from(format!("\u{0}{}", doc.buffer.content()));
    doc.saved_content = marker;
    doc.saved_version = doc.buffer.version().saturating_sub(1);
    app.recompute_dirty(id);
}
