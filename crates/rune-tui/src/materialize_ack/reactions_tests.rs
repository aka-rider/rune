#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{ClockFn, DbEvent, MatResult, Observation, OpOutcome, Store};
use rune_vfs::{Mem, Vfs};

use super::handle_materialize_ack;
use crate::app::App;
use crate::db::{Db, DbBridge, DocDb};
use crate::messages;

fn racer_observation(doc_id: i64) -> Observation {
    Observation {
        id: 1,
        doc_id,
        session_id: 1,
        blob_hash: "racer".to_string(),
        seq: None,
        size: Some(11),
        mtime: Some("t".to_string()),
        inode: None,
        device: None,
        nlink: None,
        origin: "save".to_string(),
        parent_a: None,
        parent_b: None,
        at: "t".to_string(),
        confirmed: Some(true),
    }
}

/// Builds the exact shape [`super::lost_create_race`] requires: a document
/// already bound to `path`, `bind_new: true`, and no `pending_bind_path` —
/// a named-but-unpublished document whose create is about to be told it
/// lost the race.
fn app_bound_to(mem: &Arc<Mem>, path: &str) -> (App, i64) {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = DbBridge::bootstrap();
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("store");

    store.create_scratch().expect("enqueue create_scratch");
    let row_id = match bridge.wait_for_bootstrap_event(|_| true) {
        DbEvent::Ok {
            result: OpOutcome::RowId(row_id),
            ..
        } => row_id,
        other => panic!("expected a RowId ack, got {other:?}"),
    };

    let mut app = App::new(
        Buffer::new("unpublished body"),
        Some(PathBuf::from(path)),
        vfs,
        Some(Db::new(store, Arc::clone(&bridge), false)),
    );
    app.active_doc_mut().db = Some(DocDb::new(row_id, true, 0));
    app.bind_file(row_id, 0);
    (app, row_id)
}

/// Issue #80: the hand-off's own path-identity compare must take the SAME
/// conservative branch a genuine collision-elsewhere takes when it can't
/// even resolve the racer's path — proof positive that a resolve failure
/// on the path itself, not just on some other document's binding, forces
/// `hand_off_safe = false`.
#[test]
fn a_resolve_failure_on_the_racers_own_path_keeps_the_plain_refusal() {
    let mem = Arc::new(Mem::new());
    mem.fail_resolve(std::path::Path::new("/root/nope.md"));
    let (mut app, row_id) = app_bound_to(&mem, "/root/nope.md");
    let id = app.active;

    handle_materialize_ack(
        &mut app,
        id,
        MatResult {
            committed: false,
            saved: None,
            fresh: Some(racer_observation(row_id)),
            missing: false,
            raced: false,
        },
    );

    assert!(
        messages::newest_text(&app).is_some_and(|m| m.contains("^R")),
        "got {:?}",
        messages::newest_text(&app)
    );
    assert!(
        app.doc(id).unwrap().db.as_ref().unwrap().bind_new,
        "no hand-off happened, so bind_new must stay true"
    );
    assert!(
        app.db_ops.is_empty(),
        "no Load must have been enqueued when the hand-off's own resolve failed"
    );
}

/// The companion positive control: an ordinary, resolvable racer path
/// DOES hand off (`^M` message, a Load enqueued) — proof the fixture
/// itself reaches the hand-off branch at all whenever resolution isn't
/// the thing standing in its way.
#[test]
fn a_resolvable_racer_path_hands_off_to_a_load() {
    let mem = Arc::new(Mem::new());
    let (mut app, row_id) = app_bound_to(&mem, "/root/nope.md");
    let id = app.active;

    handle_materialize_ack(
        &mut app,
        id,
        MatResult {
            committed: false,
            saved: None,
            fresh: Some(racer_observation(row_id)),
            missing: false,
            raced: false,
        },
    );

    assert!(
        messages::newest_text(&app).is_some_and(|m| m.contains("^M")),
        "got {:?}",
        messages::newest_text(&app)
    );
    assert!(
        !app.db_ops.is_empty(),
        "a resolvable racer path must hand off to a Load"
    );
}
