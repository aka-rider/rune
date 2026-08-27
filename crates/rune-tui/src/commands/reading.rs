use crate::app::App;
use crate::document::ReadOnly;
use crate::focus::{self, FocusTarget};
use crate::messages;

pub fn toggle(app: &mut App) {
    if !matches!(
        focus::target(app),
        FocusTarget::Editor | FocusTarget::Palette
    ) {
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

pub(crate) fn leave_reading(app: &mut App, id: crate::document::DocumentId) {
    let Some(doc) = app.doc_mut(id) else { return };
    if doc.read_only == ReadOnly::Reading {
        doc.read_only = ReadOnly::No;
        doc.reading_link_focus = None;
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
                install_pos: 0,
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
        app.frame = Some(crate::app::FrameSize::new(5, 5));
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
