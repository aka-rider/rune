//! The `⌃P`/`⌘P` reading-view toggle (plan WP5) — the one place that mints
//! `ReadOnly::Reading`. Flips the active document between `ReadOnly::No`
//! and `ReadOnly::Reading`; refuses on `ReadOnly::Always` and `ReadOnly::
//! Preview`, neither of which has an editable form the toggle could return
//! to, exactly like `rename::begin`'s own read-only refusal (`Document::
//! read_only`'s doc comment).
//!
//! No manual view invalidation is needed here: `Document::view()` calls
//! `set_reveal_mode` before anything else, and that transition alone marks
//! `DocMachine` dirty, so the toggle's geometry change is absorbed by the
//! very next `view()` call (`document/tests.rs`'s sync-idempotence pin).

use crate::app::{App, StatusSource};
use crate::document::ReadOnly;
use crate::pane::Pane;

/// Toggles the active document's `ReadOnly` state between `No` and
/// `Reading`. `Always`/`Preview` are left untouched with a status message —
/// the same refusal shape `rename::begin` uses for the identical
/// precondition.
pub fn toggle(app: &mut App) {
    // The reading view is a property of the document the Editor pane
    // renders, so the pane that renders it is the only pane that may
    // toggle it (CONSTITUTION §2.1) — `read_only` must never transition
    // while, say, the title field holds focus. A silent no-op, not a
    // refusal with a status message: `⌃P` firing from another pane is not
    // user-initiated intent to toggle THIS document, the same precondition
    // `app.rs::refocus_title` treats silently rather than as a refusal.
    if app.focus() != Pane::Editor {
        return;
    }
    let doc = app.active_doc_mut();
    match doc.read_only {
        ReadOnly::No => doc.read_only = ReadOnly::Reading,
        ReadOnly::Reading => doc.read_only = ReadOnly::No,
        ReadOnly::Always | ReadOnly::Preview => {
            if let Some(message) = doc.read_only.refusal_message() {
                app.set_status(message, StatusSource::Other);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
    }

    #[test]
    fn toggle_flips_an_ordinary_document_between_no_and_reading() {
        let mut app = app();
        assert_eq!(app.active_doc().read_only, ReadOnly::No);

        toggle(&mut app);
        assert_eq!(app.active_doc().read_only, ReadOnly::Reading);

        toggle(&mut app);
        assert_eq!(app.active_doc().read_only, ReadOnly::No);
    }

    #[test]
    fn toggle_refuses_on_a_document_with_no_editable_form() {
        let mut app = app();
        app.active_doc_mut().read_only = ReadOnly::Always;

        toggle(&mut app);

        assert_eq!(app.active_doc().read_only, ReadOnly::Always);
        assert_eq!(
            app.status_message.as_deref(),
            ReadOnly::Always.refusal_message()
        );
    }

    #[test]
    fn toggle_refuses_on_a_preview_document() {
        let mut app = app();
        app.active_doc_mut().read_only = ReadOnly::Preview;

        toggle(&mut app);

        assert_eq!(app.active_doc().read_only, ReadOnly::Preview);
        assert_eq!(
            app.status_message.as_deref(),
            ReadOnly::Preview.refusal_message()
        );
    }
}
