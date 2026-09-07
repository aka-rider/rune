use std::path::Path;

use rune_db::{DbEvent, SyncKind};

use crate::document::DocumentId;
use crate::find::project_replace_fixture::{
    A, B, C, FILES, StoreKind, assert_all_replaced, assert_disk_untouched, fixture,
};
use crate::find::test_support::{ALT, CTRL, SUP, escape, key, press_into};
use crate::keymap::KeyCode;
use crate::runtime::Msg;
use crate::workspace;

#[test]
fn replace_all_without_a_store_edits_three_dirty_tabs_and_leaves_the_disk_alone() {
    let mut fx = fixture(StoreKind::Absent);
    fx.prepare("dog", "cat");

    fx.replace_all();

    assert_all_replaced(&fx);
    for (path, _) in FILES {
        let id = fx.doc_for(path).expect("opened");
        assert!(fx.app.doc(id).expect("doc").is_dirty(), "{path} is dirty");
    }
    assert_disk_untouched(&fx);
    assert_eq!(fx.newest(), "replaced in 3 files");
    assert_eq!(fx.app.find_walk_queued(), None);
}

#[test]
fn replace_all_with_a_degraded_store_edits_immediately() {
    let mut fx = fixture(StoreKind::Degraded);
    fx.prepare("dog", "cat");

    fx.replace_all();

    assert_all_replaced(&fx);
    assert!(
        fx.app.db_ops.is_empty(),
        "a degraded store enqueues nothing"
    );
    assert_eq!(fx.newest(), "replaced in 3 files");
}

#[test]
fn replace_all_with_a_live_store_edits_each_document_only_on_its_load_ack() {
    let mut fx = fixture(StoreKind::Live);
    fx.prepare("dog", "cat");

    fx.replace_all();

    for (path, content) in FILES {
        assert_eq!(fx.content_of(path), *content, "{path} waits for its ack");
        assert!(fx.is_binding(path));
    }
    assert_eq!(fx.newest(), "replacing in 3 files as they load");

    let mut edited = Vec::new();
    for _ in 0..3 {
        let doc = fx.deliver_next_real_load_ack();
        edited.push(doc);
        for (path, content) in FILES {
            let id = fx.doc_for(path).expect("open");
            let now = fx.content_of(path);
            if edited.contains(&id) {
                assert_ne!(now, *content, "{path} is edited once its ack landed");
            } else {
                assert_eq!(now, *content, "{path} is untouched before its ack");
            }
        }
    }
    assert_all_replaced(&fx);
    assert_disk_untouched(&fx);
    assert_eq!(fx.newest(), "replaced in 3 files");
    assert_eq!(fx.app.find_walk_queued(), None);
}

#[test]
fn a_recovered_draft_is_hydrated_before_the_replacement_lands() {
    let mut fx = fixture(StoreKind::Silent);
    fx.prepare("dog", "cat");
    fx.replace_all();

    fx.ack_load(A, "dog one dog and a third dog", SyncKind::BufferAhead);

    assert_eq!(fx.content_of(A), "cat one cat and a third cat");
    assert_eq!(fx.content_of(B), "a dog here");
}

#[test]
fn a_diverged_recovered_draft_is_left_unchanged_and_named_in_the_summary() {
    let mut fx = fixture(StoreKind::Silent);
    fx.prepare("dog", "cat");
    fx.replace_all();

    fx.ack_clean_load(A);
    fx.ack_load(B, "a dog here, edited offline", SyncKind::Diverged);
    fx.ack_clean_load(C);

    assert_eq!(fx.content_of(A), "cat one cat");
    assert_eq!(fx.content_of(B), "a dog here, edited offline");
    assert_eq!(fx.content_of(C), "cat and cat");
    assert_eq!(
        fx.newest(),
        "replaced in 2 files, 1 skipped; b.md kept unchanged: disk changed under recovered edits, merge first"
    );
}

#[test]
fn a_document_opened_just_before_pressing_all_is_queued_until_its_ack() {
    let mut fx = fixture(StoreKind::Silent);
    fx.prepare("dog", "cat");
    workspace::open_path(&mut fx.app, Path::new(A)).expect("opens");
    assert!(fx.is_binding(A), "test setup: the load is still in flight");

    fx.replace_all();

    assert_eq!(fx.content_of(A), "dog one dog");
    assert_eq!(fx.app.find_walk_queued(), Some(3));

    fx.ack_clean_load(A);

    assert_eq!(fx.content_of(A), "cat one cat");
    assert_eq!(fx.app.find_walk_queued(), Some(2));
}

#[test]
fn toggling_case_while_acks_are_pending_does_not_change_what_queued_files_receive() {
    let mut fx = fixture(StoreKind::Silent);
    fx.prepare("dog", "cat");
    fx.replace_all();

    press_into(&mut fx.app, key(KeyCode::Char('c'), ALT), &mut fx.effects);
    assert!(fx.app.find().expect("open").options.case_sensitive);
    fx.ack_clean_load(C);

    assert_eq!(
        fx.content_of(C),
        "cat and cat",
        "the walk keeps the case-insensitive pattern it started with"
    );
}

#[test]
fn one_undo_restores_a_replaced_document() {
    let mut fx = fixture(StoreKind::Absent);
    fx.prepare("dog", "cat");
    fx.replace_all();
    press_into(&mut fx.app, escape(), &mut fx.effects);
    let c = fx.doc_for(C).expect("open");
    workspace::switch_to(&mut fx.app, c);

    press_into(&mut fx.app, key(KeyCode::Char('z'), SUP), &mut fx.effects);

    assert_eq!(fx.content_of(C), "Dog and dog");
}

#[test]
fn closing_the_panel_with_acks_outstanding_warns_and_leaves_the_documents_unedited() {
    let mut fx = fixture(StoreKind::Silent);
    fx.prepare("dog", "cat");
    fx.replace_all();

    press_into(&mut fx.app, escape(), &mut fx.effects);

    assert!(fx.app.find().is_none());
    assert_eq!(fx.newest(), "3 files were opened but not replaced");
    let untouched_ids: Vec<DocumentId> = FILES
        .iter()
        .map(|(path, _)| fx.doc_for(path).expect("still open"))
        .collect();
    for (id, (path, content)) in untouched_ids.into_iter().zip(FILES) {
        let op_id = fx.pending_load_op(id);
        crate::app::update(
            &mut fx.app,
            Msg::Db(DbEvent::Err {
                id: op_id,
                error: "store hiccup".to_string(),
            }),
            &mut fx.effects,
        );
        assert_eq!(fx.content_of(path), *content, "{path} stays unedited");
    }
}

#[test]
fn switching_scope_with_acks_outstanding_warns_and_leaves_the_documents_unedited() {
    let mut fx = fixture(StoreKind::Silent);
    fx.prepare("dog", "cat");
    fx.replace_all();

    press_into(&mut fx.app, key(KeyCode::Char('f'), CTRL), &mut fx.effects);

    assert!(fx.app.find().expect("open").project.is_none());
    assert_eq!(fx.newest(), "3 files were opened but not replaced");
    assert_eq!(fx.app.find_walk_queued(), None);
    for (path, content) in FILES {
        assert_eq!(fx.content_of(path), *content);
    }
}

#[test]
fn pressing_all_during_a_walk_reports_replace_in_progress_and_starts_nothing() {
    let mut fx = fixture(StoreKind::Silent);
    fx.prepare("dog", "cat");
    fx.replace_all();
    let tabs = fx.app.documents.len();

    fx.replace_all();

    assert_eq!(fx.newest(), "replace in progress");
    assert_eq!(fx.app.documents.len(), tabs);
    assert_eq!(fx.app.find_walk_queued(), Some(3));
}

#[test]
fn the_panel_stays_open_and_focused_after_replace_all() {
    let mut fx = fixture(StoreKind::Absent);
    fx.prepare("dog", "cat");

    fx.replace_all();

    let state = fx.app.find().expect("the panel is still open");
    assert!(state.focused);
    assert!(state.project.is_some());
    assert_eq!(state.focus, crate::find::Control::Replace);
}
