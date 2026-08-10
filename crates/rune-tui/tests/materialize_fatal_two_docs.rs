//! Review fix (round 4, finding 2): `db_dispatch::handle_db_event`'s
//! `DbEvent::Fatal` arm resolves every document `on_store_failure`'s own
//! state-aware sweep finds `Recording { published: true }` in a loop before
//! degrading the store. Each of those calls
//! may itself trigger a `saved: None` re-baseline `Load` enqueue
//! (`materialize_ack::reactions`) — with a dead writer that enqueue always
//! fails, and a degrading failure there would sweep every OTHER document
//! with `save_in_flight` still queued later in the very same loop, dropping
//! their `save_pending` before their own synthetic ack ever runs. Two
//! documents, both with a physically-committed, still-unacknowledged
//! `MaterializeRecord` in flight when the writer dies, must BOTH end up
//! clean and unreported as failed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;

use rune_db::{DbEvent, OpOutcome};
use rune_tui::runtime::{CmdKind, Msg};
use rune_vfs::Vfs;

use rename_common::{next_event, send, sup, type_text};

fn wait_for_writer_death(store: &rune_db::Store, doc_id: i64) {
    let max_attempts = 4 * rune_db::QUEUE_DEPTH;
    for attempt in 0..=max_attempts {
        match store.probe_blocking_for_test(doc_id) {
            Ok(_) => assert!(
                attempt < max_attempts,
                "writer never confirmed dead after {max_attempts} blocking probes"
            ),
            Err(rune_db::Error::WriterGone) => return,
            Err(e) => panic!("unexpected error while awaiting writer death: {e}"),
        }
    }
}

/// Drives `id` (must already be `app.active`) through ⌘S all the way to a
/// physically-committed, still-unacknowledged `MaterializeRecord` — mirrors
/// `materialize_dead_writer_reentrancy.rs`'s own single-document setup.
fn commit_but_leave_unacked(
    app: &mut rune_tui::app::App,
    bridge: &std::sync::Arc<rune_tui::db::DbBridge>,
) {
    send(app, sup('s'));
    let prep_evt = rename_common::wait_for_materialize_prep(bridge);
    let mut effects = send(app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(app, vfs_done);
}

#[test]
fn a_fatal_teardown_with_two_documents_in_flight_leaves_both_clean_and_unreported() {
    let mem = rename_common::seeded_vfs();
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    let (mut app, bridge) = rename_common::app_with_store(&mem);
    let id_a = app.active;

    // A second document, bound to the store the same way `app_with_store`
    // binds the bootstrap document — a genuine `Load` against its own file.
    let db_id_b = {
        let store = &app.db.as_ref().unwrap().store;
        store.load(Path::new("/root/b.md")).expect("enqueue load");
        match next_event(&bridge) {
            DbEvent::Ok {
                result: OpOutcome::Load(load),
                ..
            } => *load,
            other => panic!("expected a Load ack, got {other:?}"),
        }
    };
    let id_b = app.open_document(rune_core::buffer::Buffer::new("b content"));
    {
        let doc = app.doc_mut(id_b).unwrap();
        doc.file_path = Some(std::path::PathBuf::from("/root/b.md"));
        doc.db = Some(rune_tui::db::DocDb::new(db_id_b.doc_id, false, 0));
        doc.viewport
            .set_size(rename_common::WIDTH, rename_common::HEIGHT - 1);
    }
    app.install_or_join_file_binding(db_id_b.doc_id, db_id_b.saved_obs.unwrap_or(0));

    // Dirty and save BOTH documents, each up through a physically-committed
    // write whose `MaterializeRecord` ack has not landed yet.
    type_text(&mut app, "!");
    commit_but_leave_unacked(&mut app, &bridge);

    app.active = id_b;
    type_text(&mut app, "!");
    commit_but_leave_unacked(&mut app, &bridge);

    assert_eq!(
        app.doc(id_a).unwrap().save_phase(),
        rune_tui::document::SavePhase::Recording { published: true },
        "test setup: doc a's committed write must be tracked as published"
    );
    assert_eq!(
        app.doc(id_b).unwrap().save_phase(),
        rune_tui::document::SavePhase::Recording { published: true },
        "test setup: doc b's committed write must be tracked as published"
    );

    // The writer dies before either op's own reply would have arrived.
    let (store_db_id, _) = (
        app.doc(id_a).unwrap().db.as_ref().unwrap().db_id,
        app.doc(id_b).unwrap().db.as_ref().unwrap().db_id,
    );
    {
        let store = &app.db.as_ref().unwrap().store;
        store.kill_writer_for_test().expect("enqueue the kill op");
        wait_for_writer_death(store, store_db_id);
    }
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "test setup: the store must still read non-degraded going into the Fatal event"
    );

    send(
        &mut app,
        Msg::Db(DbEvent::Fatal {
            error: "writer thread died".to_string(),
        }),
    );

    assert_eq!(
        mem.read(Path::new("/root/a.md")).expect("file present"),
        b"!a content",
        "doc a's write already committed before the writer died"
    );
    assert_eq!(
        mem.read(Path::new("/root/b.md")).expect("file present"),
        b"!b content",
        "doc b's write already committed before the writer died"
    );
    assert!(
        !app.doc(id_a).unwrap().is_dirty(),
        "doc a's already-landed save must not read as dirty after the Fatal teardown"
    );
    assert!(
        !app.doc(id_b).unwrap().is_dirty(),
        "doc b's already-landed save must not read as dirty after the Fatal teardown \
         — this is the re-entrancy this test exists to catch: doc a's own re-baseline \
         must never sweep doc b's still-queued save_pending out from under it"
    );
    assert!(
        app.db.as_ref().unwrap().degraded,
        "a Fatal event must still degrade the store"
    );
    assert!(
        !rune_tui::messages::log_text(&app).contains("save failed"),
        "neither document's already-successful save may be reported as failed"
    );
}
