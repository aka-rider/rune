#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::num::NonZeroU64;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_syntax::DocumentKind;
use rune_vfs::Mem;

use crate::app::App;
use crate::document::DocumentId;
use crate::merge::{MergeIntent, MergeState};
use crate::messages;
use crate::runtime::Effects;
use crate::save::gate::{self, SaveEntry};

fn draft_app() -> App {
    App::new(Buffer::new("draft body"), None, Arc::new(Mem::new()), None)
}

/// Regression: the save gate's image rung used to refuse `⌘S` on an image
/// document with no message at all — the keypress just did nothing.
#[test]
fn materialize_is_refused_with_feedback_for_an_image_document() {
    let mut app = draft_app();
    let doc = app.active;
    app.doc_mut(doc).unwrap().kind = DocumentKind::Image;
    let posts_before = messages::posts(&app);

    let result = gate::clear(&mut app, doc, SaveEntry::Materialize);

    assert!(result.is_err());
    assert_eq!(
        messages::newest_text(&app),
        Some("images can't be edited or saved here")
    );
    assert!(
        messages::posts(&app) > posts_before,
        "an image save refusal must not be silent"
    );
}

/// Regression: the save gate's missing-document rung (a race where the
/// document closed out from under an in-flight save trigger) used to refuse
/// with no message at all either.
#[test]
fn materialize_is_refused_with_feedback_when_the_document_is_no_longer_open() {
    let mut app = draft_app();
    let missing = DocumentId(NonZeroU64::new(999_999).expect("nonzero"));
    let posts_before = messages::posts(&app);

    let result = gate::clear(&mut app, missing, SaveEntry::Materialize);

    assert!(result.is_err());
    assert_eq!(
        messages::newest_text(&app),
        Some("can't save \u{2014} that document is no longer open")
    );
    assert!(
        messages::posts(&app) > posts_before,
        "a missing-document save refusal must not be silent"
    );
}

#[test]
fn bind_new_is_refused_with_feedback_while_a_save_is_in_flight() {
    let mut app = draft_app();
    let doc = app.active;
    let content: Arc<str> = Arc::from(app.doc(doc).unwrap().buffer.content());
    let version = app.doc(doc).unwrap().buffer.version();
    let _ticket = app.doc_mut(doc).unwrap().begin_save(version, content);
    let posts_before = messages::posts(&app);

    let mut effects = Effects::default();
    let commit = crate::rename_create::bind_new(&mut app, doc, "named.md", &mut effects);

    assert_eq!(commit, crate::rename::Commit::Refused);
    assert!(effects.cmds.is_empty());
    assert!(
        messages::posts(&app) > posts_before,
        "a create dropped for an in-flight save must not be silent"
    );
}

#[test]
fn bind_new_is_refused_while_a_merge_is_unresolved_on_the_document() {
    let mut app = draft_app();
    let doc = app.active;
    app.merge = MergeState::Pending {
        doc,
        generation: crate::generation::Generation::ZERO,
        intent: MergeIntent::Merge,
    };
    let posts_before = messages::posts(&app);

    let mut effects = Effects::default();
    let commit = crate::rename_create::bind_new(&mut app, doc, "named.md", &mut effects);

    assert_eq!(commit, crate::rename::Commit::Refused);
    assert!(
        effects.cmds.is_empty(),
        "a create must never be spawned past the merge save gate"
    );
    assert!(
        messages::posts(&app) > posts_before,
        "the refusal must tell the user why the name did not commit"
    );
}
