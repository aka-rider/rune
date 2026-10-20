#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::db::{Db, DbBridge, LoadPurpose};
use rune_core::buffer::Buffer;
use rune_db::{BlobHash, ClockFn, Store};
use rune_vfs::{Mem, Vfs};
use std::sync::Arc;

fn in_memory_db() -> Db {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let store = Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
    let bridge = DbBridge::bootstrap();
    Db::new(store, bridge, false)
}

fn load_with_one_step(db_id: i64, disk: &str, recovered: &str) -> rune_db::LoadResult {
    let step = rune_core::undo::Step {
        edits: vec![AppliedEdit {
            start: disk.len(),
            end: disk.len(),
            deleted: String::new(),
            insert: recovered[disk.len()..].to_string(),
        }],
        ..rune_core::undo::Step::default()
    };
    rune_db::LoadResult {
        doc_id: rune_db::DocId(db_id),
        renamed_from: None,
        disk_content: disk.to_string(),
        recovered: rune_db::Recovered {
            content: recovered.to_string(),
            cursors: Vec::new(),
        },
        has_history: true,
        sync: rune_db::SyncState {
            kind: rune_db::SyncKind::BufferAhead,
            ancestor: None,
            ours: rune_db::Version {
                hash: BlobHash(String::new()),
                obs: None,
            },
            theirs: None,
        },
        nlink: 1,
        saved_obs: rune_db::ObsId::new(1),
        history: rune_db::Lineage {
            base_seq: rune_db::Seq(0),
            done: vec![rune_db::LineageStep {
                seq: rune_db::Seq(1),
                step,
            }],
            undone: Vec::new(),
        },
        history_losses: Vec::new(),
        resumable_merge: None,
    }
}

fn load_ack(app: &mut App, id: DocumentId, load_result: rune_db::LoadResult) {
    let issued_version = app.doc(id).expect("doc exists").buffer().version();
    handle_load_ack(
        app,
        id,
        load_result,
        Some(issued_version),
        LoadPurpose::Recover,
        &mut crate::runtime::Effects::default(),
    );
}

fn app_on(content: &str) -> App {
    App::new(
        Buffer::new(content),
        None,
        Arc::new(Mem::new()),
        Some(in_memory_db()),
    )
}

#[test]
fn a_stored_lineage_seeds_the_tab_undo_stack() {
    let mut app = app_on("hello");
    let id = app.active;

    load_ack(&mut app, id, load_with_one_step(5, "hello", "hello!"));

    assert_eq!(app.doc(id).expect("doc").journal.len(), 1);
    assert_eq!(
        messages::newest_text(&app),
        Some("recovered unsaved changes")
    );
}

#[test]
fn a_tab_whose_file_changed_since_the_store_read_it_is_not_seeded_with_the_stored_lineage() {
    let mut app = app_on("hello, edited by someone else");
    let id = app.active;

    load_ack(&mut app, id, load_with_one_step(5, "hello", "hello!"));

    assert!(
        messages::log_text(&app)
            .contains("earlier undo history unavailable \u{2014} the file changed while loading")
    );
}

#[test]
fn a_second_tab_on_a_row_already_bound_elsewhere_is_not_seeded_with_its_lineage() {
    let mut app = app_on("hello");
    let first = app.active;
    app.doc_mut(first).expect("doc").replica = Replica::Bound(DocDb::new(
        5,
        PublishMode::OverwriteExisting,
        rune_db::Seq(0),
    ));
    let second = app.open_document(Buffer::new("hello"));

    load_ack(&mut app, second, load_with_one_step(5, "hello", "hello!"));

    assert!(messages::log_text(&app).contains(
        "earlier undo history unavailable \u{2014} this file is already open in another tab"
    ));
}

#[test]
fn every_kind_of_history_loss_is_reported_to_the_user_at_its_severity() {
    use crate::messages::Severity::{Info, Warn};
    use rune_db::HistoryLoss::{PositionRepaired, RedoTailDropped, Unreadable, Unreplayable};
    for (loss, expected, severity) in [
        (Unreplayable, "could not be replayed", Warn),
        (Unreadable, "could not be read", Warn),
        (
            PositionRepaired,
            "stored undo position was out of range",
            Warn,
        ),
        (RedoTailDropped, "earlier redo steps no longer apply", Info),
    ] {
        let mut app = app_on("hello");
        let id = app.active;
        let mut load_result = load_with_one_step(5, "hello", "hello");
        load_result.history = rune_db::Lineage::default();
        load_result.history_losses = vec![loss];

        load_ack(&mut app, id, load_result);

        let reported = messages::log(&app)
            .iter()
            .find(|m| m.text.contains(expected));
        assert_eq!(
            reported.map(|m| m.severity),
            Some(severity),
            "{loss:?} in {}",
            messages::log_text(&app)
        );
    }
}
