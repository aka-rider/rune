//! One shared way every integration test forces a document dirty through a
//! REAL edit (`saved_content` is `pub(crate)` — only
//! `Document::finish_save_ok` may move the saved baseline — so an
//! integration test, which builds as a separate crate, can no longer poke
//! the field directly; it has to go through the same production path a
//! user's keystroke does). Integration test files are separate binaries, so
//! this is the one place the fixture lives rather than re-open-coding it
//! per file (`app_quit_and_dispatch.rs`, `quit_guard.rs`,
//! `db_wiring_degraded.rs`, `rename_bind.rs`, and two of `save_flow.rs`'s
//! tests pull this in via `mod dirty_common;`).
//!
//! Dirtiness is a content comparison against `saved_content`, and
//! `saved_content` is seeded from the buffer's OWN content at construction
//! time — so any edit that round-trips back to the same final bytes is, by
//! design, NOT dirty (that is the exact "edit-then-undo" fix this design landed).
//! There is therefore no sequence of edits that leaves a document both
//! dirty AND at its originally-constructed content; a caller that needs a
//! SPECIFIC final content (e.g. a byte-exact save round trip) must
//! construct the buffer EMPTY and insert the target text itself — see
//! `save_flow.rs`'s CRLF/BOM and create-on-disk tests, which do exactly
//! that instead of calling this helper.
use rune_tui::app::App;
use rune_tui::commands::edit;
use rune_tui::document::DocumentId;

/// Makes `id` genuinely dirty via the ordinary insert-char command — the
/// same `commit_edit_batch` chokepoint a keystroke goes through, so
/// `is_dirty` (a pure content comparison, not a cache) reflects the edit
/// immediately. A no-op on a `read_only` document (the chokepoint refuses
/// those outright): callers with a read-only fixture (e.g. an image
/// document) cannot use this at all, since no production path can dirty
/// one — see `image_document.rs`, which no longer calls this helper for
/// exactly that reason.
pub fn force_dirty(app: &mut App, id: DocumentId) {
    edit::insert_char(app, id, '!');
}
