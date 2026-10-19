#![allow(clippy::panic)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{DbEvent, OpOutcome};
use rune_vfs::Mem;

use super::scope::{HistoryScope, UNSTORED_NOTICE};
use super::test_support::*;
use super::*;
use crate::db::{Db, DbBridge, DocDb, PublishMode};
use crate::document::{DocumentId, Replica};
use crate::keymap::{KeyCode, KeyInput};
use crate::runtime::{CmdKind, Effects, Msg, RecentsResult};

struct Rig {
    app: App,
    bridge: Arc<DbBridge>,
    a: DocumentId,
    b: DocumentId,
}

fn alt_h() -> KeyInput {
    key(KeyCode::Char('h'), ALT)
}

fn rig() -> Rig {
    let bridge = DbBridge::bootstrap();
    let store = rune_db::Store::open_in_memory(
        Arc::new(std::time::SystemTime::now),
        Arc::new(Mem::new()),
        bridge.on_event(),
    )
    .expect("open store");
    let mut rows = Vec::new();
    for _ in 0..2 {
        store.create_scratch().expect("enqueue create_scratch");
        match bridge.wait_for_bootstrap_event(|_| true) {
            DbEvent::Ok {
                result: OpOutcome::ScratchDocId(doc_id),
                ..
            } => rows.push(doc_id.0),
            other => panic!("expected a scratch row, got {other:?}"),
        }
    }
    let mut app = app_with("alpha beta");
    app.db = Some(Db::new(store, Arc::clone(&bridge), false));
    let a = app.active;
    let b = app.open_document(Buffer::new("alpha beta"));
    for (doc, row) in [a, b].into_iter().zip(rows) {
        app.doc_mut(doc).expect("doc exists").replica =
            Replica::Bound(DocDb::new(row, PublishMode::CreateOnly, rune_db::Seq(0)));
    }
    Rig { app, bridge, a, b }
}

fn settle_history(app: &mut App, effects: Effects) {
    for cmd in effects.cmds {
        if cmd.kind() != CmdKind::SearchHistory {
            continue;
        }
        if let Some(msg) = cmd.run() {
            crate::app::update(app, msg, &mut Effects::default());
        }
    }
}

fn pending_writes(app: &App) -> usize {
    app.search_history.ops.len()
        + app.doc_search_history.ops.len()
        + app.scope_persistence.ops.len()
}

impl Rig {
    fn await_writes(&mut self) {
        while pending_writes(&self.app) > 0 {
            let evt = self.bridge.wait_for_bootstrap_event(|_| true);
            assert!(matches!(evt, DbEvent::Ok { .. }), "{evt:?}");
            crate::db_dispatch::handle_db_event(&mut self.app, evt, &mut Effects::default());
        }
    }

    fn open_in(&mut self, doc: DocumentId) {
        if self.app.find().is_some() {
            press(&mut self.app, esc_key());
        }
        self.app.active = doc;
        self.app.sync_view();
        let effects = press(&mut self.app, key(KeyCode::Char('f'), CTRL));
        settle_history(&mut self.app, effects);
    }

    fn commit_in(&mut self, doc: DocumentId, query: &str) {
        self.open_in(doc);
        type_str(&mut self.app, query);
        press(&mut self.app, enter());
        self.await_writes();
    }

    fn toggle_scope(&mut self) {
        let effects = press(&mut self.app, alt_h());
        settle_history(&mut self.app, effects);
    }

    fn history(&self) -> Vec<String> {
        find(&self.app).find.history.clone()
    }
}

#[test]
fn a_query_committed_in_one_document_is_absent_from_the_other_documents_list_but_present_in_all() {
    let mut rig = rig();
    rig.commit_in(rig.a, "alpha");

    rig.open_in(rig.b);
    assert!(rig.history().is_empty(), "doc B has not searched yet");

    rig.toggle_scope();
    assert_eq!(rig.history(), vec!["alpha".to_string()]);

    rig.open_in(rig.a);
    assert_eq!(rig.history(), vec!["alpha".to_string()]);
}

#[test]
fn the_same_query_in_two_documents_is_remembered_by_each() {
    let mut rig = rig();
    rig.commit_in(rig.a, "alpha");
    rig.commit_in(rig.b, "alpha");

    rig.open_in(rig.b);
    assert_eq!(rig.history(), vec!["alpha".to_string()]);
}

#[test]
fn the_chip_toggles_by_mouse_reloading_the_list_and_persisting_the_choice() {
    let mut rig = rig();
    rig.commit_in(rig.b, "beta");
    rig.open_in(rig.a);
    assert!(rig.history().is_empty());

    let geo = crate::layout::geometry(rig.app.frame_area(), &rig.app);
    let (_, rect) = geo
        .find_panel
        .expect("panel open")
        .chips
        .iter()
        .flatten()
        .find(|(chip, _)| chip.kind == ChipKind::HistoryScope)
        .copied()
        .expect("the history scope chip has a rect");
    let mut effects = Effects::default();
    click_into(&mut rig.app, rect.x, rect.y, &mut effects);
    settle_history(&mut rig.app, effects);

    assert_eq!(rig.app.history_scope(), HistoryScope::All);
    assert_eq!(rig.history(), vec!["beta".to_string()]);
    rig.await_writes();

    rig.app.history_scope_choice = None;
    let mut effects = Effects::default();
    scope::load_cmd(&rig.app, &mut effects);
    settle_history(&mut rig.app, effects);
    assert_eq!(
        rig.app.history_scope(),
        HistoryScope::All,
        "a fresh launch reads the stored choice"
    );
}

#[test]
fn toggling_back_restores_the_document_list_and_tells_the_user() {
    let mut rig = rig();
    rig.commit_in(rig.b, "beta");
    rig.open_in(rig.a);

    rig.toggle_scope();
    assert_eq!(
        crate::messages::newest_text(&rig.app),
        Some("search history: all documents")
    );
    rig.toggle_scope();

    assert!(rig.history().is_empty());
    assert_eq!(
        crate::messages::newest_text(&rig.app),
        Some("search history: this document")
    );
}

fn deliver(app: &mut App, generation: u64, entries: &[&str]) {
    crate::app::update(
        app,
        Msg::RecentsLoaded {
            generation,
            result: RecentsResult::Search(Ok(entries.iter().map(|e| e.to_string()).collect())),
        },
        &mut Effects::default(),
    );
}

#[test]
fn a_history_reply_issued_before_the_toggle_is_ignored() {
    let mut rig = rig();
    rig.open_in(rig.a);
    let before = find(&rig.app).history_generation;

    rig.toggle_scope();
    let after = find(&rig.app).history_generation;
    assert_ne!(before, after, "the toggle mints a new generation");

    deliver(&mut rig.app, before.raw(), &["stale"]);
    assert!(rig.history().is_empty());
    deliver(&mut rig.app, after.raw(), &["fresh"]);
    assert_eq!(rig.history(), vec!["fresh".to_string()]);
}

#[test]
fn a_document_the_store_does_not_know_yet_says_so_when_browsing_its_history() {
    let mut rig = rig();
    rig.app.doc_mut(rig.a).expect("doc").replica = binding();
    rig.open_in(rig.a);

    press(&mut rig.app, up());
    assert_eq!(
        crate::messages::newest_text(&rig.app),
        Some(UNSTORED_NOTICE)
    );

    rig.toggle_scope();
    press(&mut rig.app, up());
    assert_ne!(
        crate::messages::newest_text(&rig.app),
        Some(UNSTORED_NOTICE),
        "the global list does not depend on the document being stored"
    );
}

#[test]
fn committing_in_an_unstored_document_still_feeds_the_global_list() {
    let mut rig = rig();
    rig.app.doc_mut(rig.a).expect("doc").replica = binding();
    rig.commit_in(rig.a, "alpha");

    rig.open_in(rig.b);
    rig.toggle_scope();
    assert_eq!(rig.history(), vec!["alpha".to_string()]);
    assert_eq!(rig.app.doc_search_history.last_persisted, None);
}

fn loaded(rig: &mut Rig, result: Result<Option<String>, crate::runtime::CmdError>) {
    let mut effects = Effects::default();
    crate::app::update(&mut rig.app, Msg::HistoryScopeLoaded(result), &mut effects);
    settle_history(&mut rig.app, effects);
}

#[test]
fn a_stored_scope_loaded_at_startup_switches_the_open_panel() {
    let mut rig = rig();
    rig.commit_in(rig.b, "beta");
    rig.open_in(rig.a);

    loaded(&mut rig, Ok(Some("all".to_string())));

    assert_eq!(rig.app.history_scope(), HistoryScope::All);
    assert_eq!(rig.history(), vec!["beta".to_string()]);
}

#[test]
fn a_stored_scope_arriving_after_the_user_toggled_does_not_override_the_choice() {
    let mut rig = rig();
    rig.open_in(rig.a);
    rig.toggle_scope();

    loaded(&mut rig, Ok(Some("doc".to_string())));

    assert_eq!(rig.app.history_scope(), HistoryScope::All);
}

#[test]
fn an_unreadable_or_unknown_stored_scope_is_reported_and_the_default_stays() {
    let mut rig = rig();

    loaded(
        &mut rig,
        Err(crate::runtime::CmdError::Refused("reader gone".to_string())),
    );
    assert_eq!(
        crate::messages::newest_text(&rig.app),
        Some("search history scope not loaded: reader gone")
    );

    loaded(&mut rig, Ok(Some("banana".to_string())));
    assert_eq!(
        crate::messages::newest_text(&rig.app),
        Some("stored search history scope \"banana\" is unknown; using this document")
    );
    assert_eq!(rig.app.history_scope(), HistoryScope::Doc);
}

fn binding() -> Replica {
    Replica::Binding {
        base: String::new(),
        pending: Vec::new(),
    }
}
