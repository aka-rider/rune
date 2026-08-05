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
use crate::viewport::ScrollMode;

/// Toggles the active document's `ReadOnly` state between `No` and
/// `Reading`. `Always`/`Preview` are left untouched with a status message —
/// the same refusal shape `rename::begin` uses for the identical
/// precondition.
pub fn toggle(app: &mut App) {
    // The reading view is a property of the document the Editor pane
    // renders, so the pane that renders it is the only pane that may
    // toggle it — `read_only` must never transition while, say, the
    // title field holds focus. A silent no-op, not a
    // refusal with a status message: `⌃P` firing from another pane is not
    // user-initiated intent to toggle THIS document, the same precondition
    // `app.rs::refocus_title` treats silently rather than as a refusal.
    if app.focus() != Pane::Editor {
        return;
    }
    // Review fix F9: while the merge resolver is `Active` ON the active
    // document, a `Reading` document makes `[O]urs`/`[T]heirs` fail
    // confusingly (`merge/keys.rs::intercept` still swallows the chords,
    // but the working form can no longer be edited to reflect them) — the
    // same "finish the merge first" refusal shape as the ⌘S gate
    // (`merge::refuses_save`).
    if matches!(app.merge, crate::merge::MergeState::Active { doc, .. } if doc == app.active) {
        app.set_status("finish or close the merge first", StatusSource::Other);
        return;
    }
    let doc = app.active_doc_mut();
    match doc.read_only {
        ReadOnly::No => doc.read_only = ReadOnly::Reading,
        ReadOnly::Reading => {
            doc.read_only = ReadOnly::No;
            // While reading, every motion key moved `scroll_row` directly
            // and the cursor was never chased, so it can now be off-screen
            // entirely — where the default `FollowCursor` settle would yank
            // the view off the page the user was just reading. `Independent`
            // inverts that: the CURSOR snaps onto the visible band instead,
            // so the caret reappears where they were reading. Armed only
            // when the cursor really is off-screen, because the snap moves
            // the cursor to the scrolloff band's edge — doing that to a
            // cursor the user can already see would drag it off whatever it
            // was resting on.
            if !doc.cursor_on_screen() {
                doc.viewport.mode = ScrollMode::Independent;
            }
        }
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

    /// Review fix F9: `^P` while the merge resolver is `Active` ON the
    /// active document must refuse — a `Reading` document would leave
    /// `[O]urs`/`[T]heirs` unable to touch the working form.
    #[test]
    fn toggle_refuses_while_the_merge_resolver_is_active_on_the_active_document() {
        let mut app = app();
        let doc = app.active;
        app.merge = crate::merge::MergeState::Active {
            doc,
            conflicts: Vec::new(),
            blocks: Vec::new(),
            cur: 0,
            saved_display_name: None,
        };

        toggle(&mut app);

        assert_eq!(app.active_doc().read_only, ReadOnly::No);
        assert!(
            app.status_message
                .as_deref()
                .unwrap_or_default()
                .contains("finish or close the merge"),
            "expected the merge refusal status, got {:?}",
            app.status_message
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
