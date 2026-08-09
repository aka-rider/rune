//! A doc-scoped read op failing (a probe against an externally deleted
//! file) surfaces a per-document error message and leaves the recovery
//! store trusted; only real journal failures degrade it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;
use std::sync::Arc;

use rune_db::DbEvent;
use rune_tui::app::{self, App};
use rune_tui::db::DbBridge;
use rune_tui::document::DocumentId;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{app_with_store, press, publish};

fn drain_one_op_for(app: &mut App, bridge: &DbBridge, doc: DocumentId) -> DbEvent {
    let op_id = *app
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == doc)
        .expect("one op recorded for this document")
        .0;
    let evt = bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    });
    let mut effects = Effects::default();
    app::update(app, Msg::Db(evt.clone()), &mut effects);
    evt
}

#[test]
fn probe_missing_file_keeps_store() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("probe-missing-file", Arc::clone(&vfs));
    let draft_id = app.active;

    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    assert_ne!(doc_id, draft_id);
    let load_evt = drain_one_op_for(&mut app, &bridge, doc_id);
    assert!(
        matches!(load_evt, DbEvent::Ok { .. }),
        "test setup: the Load ack must succeed, got {load_evt:?}"
    );
    assert!(app.db_ops.is_empty(), "test setup: Load ack fully drained");

    vfs.remove(Path::new("/doc.md"))
        .expect("delete the file out from under the open document");

    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    let posts_before = rune_tui::messages::posts(&app);
    let probe_evt = drain_one_op_for(&mut app, &bridge, doc_id);
    assert!(
        matches!(probe_evt, DbEvent::Err { .. }),
        "probing a deleted file must fail, got {probe_evt:?}"
    );

    assert!(
        rune_tui::messages::posts(&app) > posts_before,
        "the probe failure must post a message"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|s| s.contains("doc.md")),
        "the error must name the document, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
    assert!(
        app.db.as_ref().is_some_and(|d| !d.degraded),
        "a doc-scoped probe failure must never degrade the whole store"
    );
    assert!(
        app.db_banner.is_none(),
        "no sticky degraded banner for a missing file, got {:?}",
        app.db_banner
    );

    press(&mut app, '!');
    assert!(
        app.doc(doc_id).unwrap().buffer.content().contains('!'),
        "editing must keep working after the failed probe, got {:?}",
        app.doc(doc_id).unwrap().buffer.content()
    );
    let append_evt = drain_one_op_for(&mut app, &bridge, doc_id);
    assert!(
        matches!(
            append_evt,
            DbEvent::Ok {
                result: rune_db::OpOutcome::Seq(_),
                ..
            }
        ),
        "a subsequent journal append must still ack Ok, got {append_evt:?}"
    );
    assert!(
        app.db.as_ref().is_some_and(|d| !d.degraded),
        "the store must stay trusted for recovery after the whole sequence"
    );
}
