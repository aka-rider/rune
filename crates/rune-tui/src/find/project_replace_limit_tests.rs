use std::path::Path;

use rune_vfs::Vfs;

use crate::document::ReadOnly;
use crate::find::project_replace_fixture::{
    A, B, C, FILES, Fixture, StoreKind, assert_all_replaced, fixture, fixture_with,
};
use crate::find::test_support::{deliver_query, enter, press_into, project, project_grid};
use crate::workspace;

#[test]
fn the_tab_limit_stops_the_walk_and_a_second_press_finishes_it() {
    let mut fx = fixture(StoreKind::Absent);
    let origin = fx.app.active;
    fx.app.doc_mut(origin).expect("origin").pinned = true;
    let extra = fx.open_extra_docs(8, true);
    workspace::switch_to(&mut fx.app, origin);
    fx.prepare("dog", "cat");

    fx.replace_all();

    assert_eq!(
        fx.newest(),
        "replaced in 1 of 3 files; free a tab, then press All again"
    );
    assert_eq!(fx.app.documents.len(), 10);
    let replaced: Vec<&str> = FILES
        .iter()
        .map(|(path, _)| *path)
        .filter(|path| fx.doc_for(path).is_some())
        .collect();
    assert_eq!(replaced.len(), 1);
    assert!(fx.app.find().expect("open").focused);

    for id in extra.iter().take(2) {
        assert!(matches!(
            workspace::close_now(&mut fx.app, *id, &mut fx.effects),
            workspace::CloseOutcome::Closed
        ));
    }
    deliver_query(&mut fx.app, &mut fx.effects);
    assert_eq!(
        project(&fx.app).results.len(),
        2,
        "the replaced file drops out of the results"
    );

    fx.replace_all();

    assert_eq!(fx.newest(), "replaced in 2 files");
    assert_all_replaced(&fx);
    deliver_query(&mut fx.app, &mut fx.effects);
    assert!(project(&fx.app).results.is_empty());
}

#[test]
fn a_walk_holding_a_queued_document_stops_at_the_limit_instead_of_evicting_it() {
    let mut fx = fixture(StoreKind::Silent);
    let origin = fx.app.active;
    let extra = fx.open_extra_docs(8, false);
    workspace::switch_to(&mut fx.app, origin);
    fx.prepare("dog", "cat");

    fx.replace_all();

    assert_eq!(fx.app.documents.len(), 10);
    assert!(
        extra.iter().all(|id| fx.app.documents.contains_key(id)),
        "no clean tab is evicted while a queued document waits"
    );
    assert_eq!(fx.app.find_walk_queued(), Some(1));
    assert_eq!(fx.newest(), "replacing in 1 file as they load");

    let opened = FILES
        .iter()
        .map(|(path, _)| *path)
        .find(|path| fx.doc_for(path).is_some())
        .expect("one hit file is open");
    fx.ack_clean_load(opened);

    assert_ne!(fx.content_of(opened), Fixture::seed_content(opened));
    assert_eq!(
        fx.newest(),
        "replaced in 1 of 3 files; free a tab, then press All again"
    );
}

#[test]
fn a_hit_file_deleted_after_indexing_is_skipped_and_counted() {
    let mut fx = fixture(StoreKind::Absent);
    fx.prepare("dog", "cat");
    fx.mem.remove(Path::new(B)).expect("delete b.md");

    fx.replace_all();

    assert_eq!(fx.content_of(A), "cat one cat");
    assert_eq!(fx.content_of(C), "cat and cat");
    assert!(fx.doc_for(B).is_none());
    assert_eq!(fx.newest(), "replaced in 2 files, 1 skipped");
}

#[test]
fn enter_in_the_replace_field_replaces_only_the_selected_file() {
    let mut fx = fixture(StoreKind::Absent);
    fx.prepare("dog", "cat");
    let selected = project(&fx.app)
        .selected()
        .expect("a selected hit")
        .path
        .to_string_lossy()
        .into_owned();

    press_into(&mut fx.app, enter(), &mut fx.effects);

    assert!(!fx.content_of(&selected).contains("dog"));
    for (path, content) in FILES {
        if *path != selected {
            assert!(fx.doc_for(path).is_none(), "{path} is not opened");
            assert_eq!(fx.disk(path), *content);
        }
    }
    assert_eq!(fx.newest(), "replaced in 1 file");
}

#[test]
fn a_read_only_hit_file_is_skipped_and_counted() {
    let mut fx = fixture(StoreKind::Absent);
    fx.prepare("dog", "cat");
    let a = workspace::open_path(&mut fx.app, Path::new(A)).expect("opens");
    fx.app.doc_mut(a).expect("doc").read_only = ReadOnly::Always;

    fx.replace_all();

    assert_eq!(fx.content_of(A), "dog one dog");
    assert_eq!(fx.content_of(B), "a cat here");
    assert_eq!(fx.content_of(C), "cat and cat");
    assert_eq!(fx.newest(), "replaced in 2 files, 1 skipped");
}

#[test]
fn an_origin_evicted_during_the_walk_is_named_in_the_summary() {
    let mut fx = fixture_with(StoreKind::Absent, Some("/other/origin.md"));
    let origin = fx.app.active;
    fx.open_extra_docs(8, true);
    workspace::switch_to(&mut fx.app, origin);
    fx.prepare("dog", "cat");

    fx.replace_all();

    assert!(!fx.app.documents.contains_key(&origin));
    assert_eq!(
        fx.newest(),
        "replaced in 2 of 3 files; free a tab, then press All again; the tab you started from was closed to make room"
    );
}

#[test]
fn the_all_chip_shows_the_file_count_in_project_scope() {
    let mut fx = fixture(StoreKind::Absent);
    fx.prepare("dog", "cat");

    let rows = project_grid(&mut fx.app);

    assert!(
        rows.iter()
            .any(|row| row.contains("\u{21e7}\u{23ce} All 3")),
        "the All chip names the blast radius: {rows:?}"
    );
}
