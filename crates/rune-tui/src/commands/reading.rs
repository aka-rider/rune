//! The `⌃P`/`⌘P` reading-view toggle (plan WP5) — the one place that mints
//! `ReadOnly::Reading`. Flips the active document between `ReadOnly::No`
//! and `ReadOnly::Reading`; refuses on `ReadOnly::Always`, which has no
//! editable form to return to, exactly like `rename::begin`'s own
//! read-only refusal (`Document::read_only`'s doc comment).
//!
//! No manual view invalidation is needed here: `Document::view()` calls
//! `set_reveal_mode` before anything else, and that transition alone marks
//! `DocMachine` dirty, so the toggle's geometry change is absorbed by the
//! very next `view()` call (`document/tests.rs`'s sync-idempotence pin).

use crate::app::{App, StatusSource};
use crate::document::ReadOnly;

/// Toggles the active document's `ReadOnly` state between `No` and
/// `Reading`. `Always` is left untouched with a status message — the same
/// refusal shape `rename::begin` uses for the identical precondition.
pub fn toggle(app: &mut App) {
    let doc = app.active_doc_mut();
    match doc.read_only {
        ReadOnly::No => doc.read_only = ReadOnly::Reading,
        ReadOnly::Reading => doc.read_only = ReadOnly::No,
        ReadOnly::Always => {
            app.set_status(ReadOnly::Always.refusal_message(), StatusSource::Other);
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
            Some(ReadOnly::Always.refusal_message())
        );
    }
}
