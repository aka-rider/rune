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

use crate::app::App;
use crate::document::ReadOnly;
use crate::focus::{self, FocusTarget};
use crate::messages;

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
    if focus::target(app) != FocusTarget::Editor {
        return;
    }
    if matches!(app.merge, crate::merge::MergeState::Active { doc, .. } if doc == app.active) {
        messages::warn(app, "finish or close the merge first");
        return;
    }
    let doc = app.active_doc_mut();
    match doc.read_only {
        ReadOnly::No => doc.read_only = ReadOnly::Reading,
        ReadOnly::Reading => {
            doc.read_only = ReadOnly::No;
            doc.reading_link_focus = None;
        }
        ReadOnly::Always | ReadOnly::Preview => {
            if let Some(message) = doc.read_only.refusal_message() {
                messages::warn(app, message);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::pane::Pane;
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
            crate::messages::newest_text(&app),
            ReadOnly::Always.refusal_message()
        );
    }

    #[test]
    fn toggle_refuses_while_the_merge_resolver_is_active_on_the_active_document() {
        let mut app = app();
        let doc = app.active;
        app.merge = crate::merge::MergeState::Active {
            doc,
            session: crate::merge::MergeSession {
                conflicts: Vec::new(),
                cur: 0,
                saved_display_name: None,
                theirs_obs: rune_db::ObsId::new(1).expect("nonzero"),
            },
        };

        toggle(&mut app);

        assert_eq!(app.active_doc().read_only, ReadOnly::No);
        assert!(
            crate::messages::newest_text(&app)
                .unwrap_or_default()
                .contains("finish or close the merge"),
            "expected the merge refusal status, got {:?}",
            crate::messages::newest_text(&app)
        );
    }

    #[test]
    fn toggle_refuses_while_the_file_finder_owns_focus_at_a_narrow_frame() {
        let mut app = app();
        app.frame_width = 5;
        app.frame_height = 5;
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        assert_eq!(app.focus(), Pane::Editor);
        assert_eq!(
            crate::focus::target(&app),
            crate::focus::FocusTarget::FileSearch
        );

        toggle(&mut app);

        assert_eq!(app.active_doc().read_only, ReadOnly::No);
    }

    #[test]
    fn toggle_refuses_on_a_preview_document() {
        let mut app = app();
        app.active_doc_mut().read_only = ReadOnly::Preview;

        toggle(&mut app);

        assert_eq!(app.active_doc().read_only, ReadOnly::Preview);
        assert_eq!(
            crate::messages::newest_text(&app),
            ReadOnly::Preview.refusal_message()
        );
    }
}
