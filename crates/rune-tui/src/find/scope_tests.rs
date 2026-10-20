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
    rows: [i64; 2],
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
    for (doc, row) in [a, b].into_iter().zip(rows.iter().copied()) {
        app.doc_mut(doc).expect("doc exists").replica =
            Replica::Bound(DocDb::new(row, PublishMode::CreateOnly, rune_db::Seq(0)));
    }
    Rig {
        app,
        bridge,
        a,
        b,
        rows: [rows[0], rows[1]],
    }
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
    fn row(&self, doc: DocumentId) -> i64 {
        if doc == self.a {
            self.rows[0]
        } else {
            self.rows[1]
        }
    }

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

    fn click_into_the_editor(&mut self) {
        let geo = crate::layout::geometry(self.app.frame_area(), &self.app);
        click(&mut self.app, geo.editor.x, geo.editor.y);
        assert!(
            !find(&self.app).focused,
            "test setup: the panel stays open but unfocused"
        );
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

fn switch_tab_via_the_tabs_pane(rig: &mut Rig, target: DocumentId) {
    let mut effects = Effects::default();
    crate::pane_global::focus_tabs(&mut rig.app, &mut effects);
    let want = rig
        .app
        .documents
        .order()
        .iter()
        .position(|&id| id == target)
        .expect("the target tab is listed");
    while rig.app.tabs.nav.cursor != want {
        let step = if rig.app.tabs.nav.cursor < want {
            down()
        } else {
            up()
        };
        press(&mut rig.app, step);
    }
    let effects = press(&mut rig.app, enter());
    settle_history(&mut rig.app, effects);
}

#[test]
fn switching_tabs_under_an_open_panel_swaps_in_the_new_documents_history() {
    let mut rig = rig();
    rig.commit_in(rig.a, "alpha");
    rig.commit_in(rig.b, "beta");
    rig.open_in(rig.a);
    assert_eq!(rig.history(), vec!["alpha".to_string()]);
    rig.click_into_the_editor();

    let b = rig.b;
    switch_tab_via_the_tabs_pane(&mut rig, b);

    assert_eq!(find(&rig.app).origin.doc, rig.b);
    assert_eq!(rig.history(), vec!["beta".to_string()]);
    assert_eq!(
        rig.app.doc_search_history.last_persisted,
        Some((rune_db::DocId(rig.row(rig.b)), "beta".to_string())),
        "the history shown and the list a commit feeds are the same document's"
    );
}

#[test]
fn closing_the_origin_document_under_an_open_panel_repoints_the_panel_and_its_history() {
    let mut rig = rig();
    rig.commit_in(rig.b, "beta");
    rig.open_in(rig.a);
    rig.click_into_the_editor();

    let mut effects = Effects::default();
    crate::workspace::request_close(&mut rig.app, rig.a, &mut effects);
    crate::app::update(&mut rig.app, Msg::PumpGraphics, &mut effects);
    settle_history(&mut rig.app, effects);

    assert_eq!(rig.app.active, rig.b);
    assert_eq!(find(&rig.app).origin.doc, rig.b, "no dead origin is kept");
    assert_eq!(rig.history(), vec!["beta".to_string()]);
}

#[test]
fn a_document_stored_after_the_panel_opened_gets_its_history_loaded() {
    let mut rig = rig();
    rig.commit_in(rig.a, "alpha");
    let row = rig.row(rig.a);
    rig.app.doc_mut(rig.a).expect("doc").replica = binding();
    rig.open_in(rig.a);
    assert!(rig.history().is_empty(), "nothing loads while unstored");

    rig.app.doc_mut(rig.a).expect("doc").replica =
        Replica::Bound(DocDb::new(row, PublishMode::CreateOnly, rune_db::Seq(0)));
    let effects = press(&mut rig.app, key(KeyCode::Char('x'), CTRL));
    settle_history(&mut rig.app, effects);

    assert_eq!(rig.history(), vec!["alpha".to_string()]);
}

#[test]
fn committing_in_an_unstored_document_says_so_right_away() {
    let mut rig = rig();
    rig.app.doc_mut(rig.a).expect("doc").replica = binding();
    rig.open_in(rig.a);
    type_str(&mut rig.app, "alpha");

    press(&mut rig.app, enter());

    assert_eq!(
        crate::messages::newest_text(&rig.app),
        Some(UNSTORED_NOTICE)
    );
}

#[test]
fn toggling_the_scope_without_a_store_says_the_choice_is_not_saved() {
    let mut app = app_with("alpha");
    open_find(&mut app);

    press(&mut app, alt_h());

    assert_eq!(app.history_scope(), HistoryScope::All);
    assert!(
        crate::messages::newest_text(&app)
            .is_some_and(|text| text.starts_with("search history scope not saved")),
        "{:?}",
        crate::messages::newest_text(&app)
    );
}

#[test]
fn toggling_the_scope_with_a_store_does_not_cry_wolf() {
    let mut rig = rig();
    rig.open_in(rig.a);

    rig.toggle_scope();
    rig.toggle_scope();
    rig.await_writes();

    assert_eq!(
        crate::messages::newest_text(&rig.app),
        Some("search history: this document")
    );
}
