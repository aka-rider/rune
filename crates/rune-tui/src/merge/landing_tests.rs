#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::*;
use rune_core::buffer::Buffer;
use rune_core::coords::BufferOffset;
use rune_vfs::Mem;
use std::sync::Arc;

fn app_with(content: &str) -> App {
    App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
}

#[test]
fn discard_install_replaces_the_buffer_with_theirs_verbatim_and_never_touches_ancestor() {
    let mut app = app_with("ours text the discard replaces\n");
    let doc = app.active;

    discard_install(
        &mut app,
        doc,
        "disk replacement\n",
        rune_db::ObsId::new(42).expect("nonzero"),
    );

    assert_eq!(app.doc(doc).unwrap().buffer.content(), "disk replacement\n");
    assert_eq!(app.merge, MergeState::Inactive);
    assert_eq!(messages::newest_text(&app), Some("disk changes adopted"));
}

#[test]
fn ack_refuses_cleanly_when_sync_claims_diverged_but_theirs_is_absent() {
    let mut app = app_with("hello");
    let doc = app.active;
    app.merge = MergeState::Pending {
        doc,
        generation: crate::generation::Generation::ZERO,
        intent: MergeIntent::Merge,
    };

    let prep = MergePrepResult {
        sync: rune_db::SyncState {
            kind: SyncKind::Diverged,
            ancestor: None,
            ours: rune_db::Version {
                hash: BlobHash(String::new()),
                obs: None,
            },
            theirs: None,
        },
        outcome: MergePrepOutcome::Ready {
            ancestor: None,
            theirs: None,
        },
    };

    let mut effects = Effects::default();
    handle_merge_prep_ack(
        &mut app,
        doc,
        Some(crate::generation::Generation::ZERO),
        prep,
        &mut effects,
    );

    assert_eq!(app.merge, MergeState::Inactive);
    assert!(
        messages::newest_text(&app)
            .unwrap_or_default()
            .contains("no disk version"),
        "expected the F4 refusal status, got {:?}",
        messages::newest_text(&app)
    );
}

fn diverged_prep(theirs: &[u8], theirs_obs: ObsId) -> MergePrepResult {
    MergePrepResult {
        sync: rune_db::SyncState {
            kind: SyncKind::Diverged,
            ancestor: None,
            ours: rune_db::Version {
                hash: BlobHash(String::new()),
                obs: None,
            },
            theirs: None,
        },
        outcome: MergePrepOutcome::Ready {
            ancestor: None,
            theirs: Some((theirs_obs, theirs.to_vec())),
        },
    }
}

#[test]
fn absent_ancestor_notifies_and_localizes_via_the_2way_path() {
    let mut app = app_with("shared-start\nours-only\nshared-end\n");
    let doc = app.active;
    app.merge = MergeState::Pending {
        doc,
        generation: crate::generation::Generation::ZERO,
        intent: MergeIntent::Merge,
    };

    let mut effects = Effects::default();
    handle_merge_prep_ack(
        &mut app,
        doc,
        Some(crate::generation::Generation::ZERO),
        diverged_prep(
            b"shared-start\ntheirs-only\nshared-end\n",
            rune_db::ObsId::new(3).expect("nonzero"),
        ),
        &mut effects,
    );

    assert!(
        messages::log_text(&app).contains("no saved ancestor"),
        "expected the absent-ancestor notice, got {:?}",
        messages::log_text(&app)
    );
    let MergeState::Active { session, .. } = &app.merge else {
        panic!("expected an Active merge, got {:?}", app.merge);
    };
    assert_eq!(
        session.conflicts.len(),
        1,
        "expected exactly one localized conflict, not a whole-file collapse"
    );
    let buffer = app.doc(doc).unwrap().buffer.content().to_string();
    assert!(
        buffer.starts_with("shared-start\n"),
        "clean prefix lost: {buffer:?}"
    );
    assert!(
        buffer.ends_with("shared-end\n"),
        "clean suffix lost: {buffer:?}"
    );
}

#[test]
fn a_save_in_flight_cancels_the_landing_instead_of_installing_a_resolver() {
    let mut app = app_with("shared-start\nours-only\nshared-end\n");
    let doc = app.active;
    app.merge = MergeState::Pending {
        doc,
        generation: crate::generation::Generation::ZERO,
        intent: MergeIntent::Merge,
    };
    let content: Arc<str> = Arc::from(app.doc(doc).unwrap().buffer.content());
    let version = app.doc(doc).unwrap().buffer.version();
    let _ticket = app.doc_mut(doc).unwrap().begin_save(version, content);
    assert!(app.doc(doc).unwrap().save_in_flight());
    let before = app.doc(doc).unwrap().buffer.content().to_string();

    let mut effects = Effects::default();
    handle_merge_prep_ack(
        &mut app,
        doc,
        Some(crate::generation::Generation::ZERO),
        diverged_prep(
            b"shared-start\ntheirs-only\nshared-end\n",
            rune_db::ObsId::new(11).expect("nonzero"),
        ),
        &mut effects,
    );

    assert_eq!(app.merge, MergeState::Inactive);
    assert_eq!(
        app.doc(doc).unwrap().buffer.content(),
        before,
        "no working form may be installed under an in-flight save"
    );
    assert!(
        messages::log_text(&app).contains("save in flight"),
        "expected the cancelled-merge notice, got {:?}",
        messages::log_text(&app)
    );
}

#[test]
fn nothing_to_merge_refusal_records_the_fresh_classification() {
    let mut app = app_with("hello");
    let doc = app.active;
    app.merge = MergeState::Pending {
        doc,
        generation: crate::generation::Generation::ZERO,
        intent: MergeIntent::Merge,
    };
    let prep = MergePrepResult {
        sync: rune_db::SyncState {
            kind: SyncKind::Clean,
            ancestor: None,
            ours: rune_db::Version {
                hash: BlobHash(String::new()),
                obs: None,
            },
            theirs: None,
        },
        outcome: MergePrepOutcome::Ready {
            ancestor: None,
            theirs: None,
        },
    };

    let mut effects = Effects::default();
    handle_merge_prep_ack(
        &mut app,
        doc,
        Some(crate::generation::Generation::ZERO),
        prep,
        &mut effects,
    );

    assert_eq!(app.merge, MergeState::Inactive);
    assert_eq!(app.doc(doc).unwrap().last_sync, Some(SyncKind::Clean));
}

#[test]
fn discard_install_success_marks_the_document_clean() {
    let mut app = app_with("ours");
    let doc = app.active;

    discard_install(
        &mut app,
        doc,
        "theirs\n",
        rune_db::ObsId::new(1).expect("nonzero"),
    );

    assert_eq!(app.doc(doc).unwrap().last_sync, Some(SyncKind::Clean));
    assert_eq!(app.merge, MergeState::Inactive);
}

#[test]
fn failed_install_leaves_expect_obs_and_last_sync_untouched() {
    let mut app = app_with("hello");
    let doc = app.active;
    if let Some(d) = app.doc_mut(doc) {
        d.read_only = crate::document::ReadOnly::Always;
        d.replica = crate::document::Replica::Bound(crate::db::DocDb::new(
            1,
            crate::db::PublishMode::OverwriteExisting,
            rune_db::Seq(0),
        ));
    }
    app.install_or_join_file_binding(1, Some(rune_db::ObsId::new(7).expect("nonzero")));
    app.merge = MergeState::Pending {
        doc,
        generation: crate::generation::Generation::ZERO,
        intent: MergeIntent::Merge,
    };

    let mut effects = Effects::default();
    handle_merge_prep_ack(
        &mut app,
        doc,
        Some(crate::generation::Generation::ZERO),
        diverged_prep(b"disk\n", rune_db::ObsId::new(9).expect("nonzero")),
        &mut effects,
    );

    assert_eq!(app.merge, MergeState::Inactive);
    assert_eq!(app.doc(doc).unwrap().buffer.content(), "hello");
    assert_eq!(app.doc(doc).unwrap().last_sync, None);
    assert_eq!(
        app.file_binding(1).unwrap().expect_obs,
        Some(rune_db::ObsId::new(7).expect("nonzero"))
    );
    assert!(
        messages::newest_text(&app)
            .unwrap_or_default()
            .contains("merge failed"),
        "expected the failed-install status, got {:?}",
        messages::newest_text(&app)
    );
}

#[test]
fn begin_refuses_while_a_save_is_in_flight() {
    let mut app = app_with("hello");
    let doc = app.active;
    if let Some(d) = app.doc_mut(doc) {
        d.last_sync = Some(SyncKind::Diverged);
        d.begin_save(d.buffer.version(), Arc::from(d.buffer.content()));
    }

    let mut effects = Effects::default();
    crate::merge::begin(&mut app, MergeIntent::Merge, &mut effects);

    assert_eq!(app.merge, MergeState::Inactive);
    assert!(
        messages::newest_text(&app)
            .unwrap_or_default()
            .contains("save in progress"),
        "expected the save-in-flight refusal, got {:?}",
        messages::newest_text(&app)
    );
}

#[test]
fn merge_result_would_erase_nonempty_ours_flags_only_that_case() {
    assert!(merge_result_would_erase_nonempty_ours("", "ours had text"));
    assert!(!merge_result_would_erase_nonempty_ours("", ""));
    assert!(!merge_result_would_erase_nonempty_ours(
        "merged text",
        "ours had text"
    ));
    assert!(!merge_result_would_erase_nonempty_ours("merged text", ""));
}

#[test]
fn install_whole_range_places_the_cursor_at_the_requested_offset() {
    let mut app = app_with("old content");
    let doc = app.active;

    assert!(install_whole_range(&mut app, doc, "new content", 4));

    let cursors = app.doc(doc).unwrap().cursors.all();
    assert_eq!(cursors.len(), 1);
    assert_eq!(cursors[0].position, BufferOffset(4));
    assert_eq!(cursors[0].anchor, BufferOffset(4));
}
