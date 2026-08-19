#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use crate::app::App;
use crate::merge::{MergeIntent, MergeState};
use crate::messages;
use crate::runtime::Effects;

fn draft_app() -> App {
    App::new(Buffer::new("draft body"), None, Arc::new(Mem::new()), None)
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
