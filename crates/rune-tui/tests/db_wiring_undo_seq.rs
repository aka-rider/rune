//! Regression for the deferred-resolution `MoveUndoPos` rework: a typing
//! burst that leaves several `AppendEdit` acks in flight, followed by an
//! undo whose `MoveUndoPos` enqueues BEFORE those acks are drained, must
//! never desynchronize the durable journal from the live buffer — the
//! writer thread resolves the undo target itself, from ops it has already
//! executed, never from this app's own lagging `last_known_seq` estimate.
//! Driven through `rune_fuzz::Session`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;
use std::sync::Arc;

use rune_db::{DbEvent, OpOutcome};
use rune_fuzz::Session;
use rune_fuzz::driver::wait_for_db_op;
use rune_tui::db::Db;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{publish, restarted_store_at, store_at, temp_db_dir};

const UNDO: KeyInput = KeyInput {
    code: KeyCode::Char('z'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};

/// Data-safety regression: an undo committed while several of this
/// session's own `AppendEdit` acks are still in flight must resolve to the
/// EXACT durable seq the writer thread already applied — never an estimate
/// this app-side lagging bookkeeping guesses at — so the durable journal
/// never truncates events the buffer itself never undid (the corruption
/// chain the pinned `save-inflight-sm` fuzz artifact reproduced).
#[test]
fn undo_committed_with_unacked_appends_in_flight_never_desyncs_the_durable_journal() {
    let dir = temp_db_dir("undo-seq");
    let db_path = dir.join("rune-v1.db");
    let doc_path = Path::new("/doc.md");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), doc_path, b"");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    let (store, bridge) = store_at(&db_path, Arc::clone(&vfs));
    let mut session =
        Session::open_with_db("/doc.md", Arc::clone(&mem), Db::new(store, bridge, false));

    // Three whole-word pastes: each is ONE journal push and ONE `AppendEdit`
    // enqueue (`edit_core::insert_text`'s multi-char batch never coalesces
    // durably — `journal_append::as_single_char_insert` only coalesces a
    // SINGLE-char insert), so each maps to its own distinct durable seq —
    // no ambiguity from the durable side's typing-run coalescing muddying
    // what this test is isolating.
    for word in ["alpha", "beta", "gamma"] {
        assert!(session.paste(word).is_none());
    }
    assert_eq!(session.snapshot().content, "alphabetagamma");
    assert_eq!(
        session.app().db_ops.len(),
        3,
        "three AppendEdit ops in flight"
    );

    // Deliver ONLY the FIRST (oldest) ack — "alpha" — a typing burst leaving
    // the other two ("beta", "gamma") still unacknowledged, exactly the
    // `last_known_seq`-lagging window the bug chain started from.
    assert!(session.deliver_db().is_none());
    assert_eq!(
        session.app().db_ops.len(),
        2,
        "beta/gamma acks still in flight"
    );

    // Undo once: removes "gamma" from the LOCAL buffer/journal. Its
    // `MoveUndoPos` enqueues while "beta"'s AND "gamma"'s own `AppendEdit`
    // acks are still undelivered — the writer thread has already EXECUTED
    // both (strict FIFO), so it can resolve this exactly; only this app's
    // OWN bookkeeping is behind.
    assert!(session.key(UNDO).is_none());
    assert_eq!(session.snapshot().content, "alphabeta");
    assert!(
        !session.app().db.as_ref().unwrap().degraded,
        "MoveUndoPos must not fail against unacked in-flight AppendEdits"
    );

    // One new edit after the undo — the exact "next AppendEdit sees a
    // corrupted current_seq" step in the bug chain: if `MoveUndoPos` above
    // had resolved to an underestimate, this append would truncate durable
    // events it never should have.
    assert!(session.paste("delta").is_none());
    assert_eq!(session.snapshot().content, "alphabetadelta");

    // Drain every remaining in-flight ack — including the pre-undo "beta"/
    // "gamma" acks and the post-undo ops. A truncated/corrupted journal
    // surfaces exactly there, via `edit out of bounds`/`current_seq`
    // mismatches on the writer side — which would degrade the store or
    // post an error message.
    assert!(session.deliver_db_all().is_none());
    assert!(
        !session.app().db.as_ref().unwrap().degraded,
        "the store must still be healthy once every op has been acked"
    );
    assert_eq!(
        rune_tui::messages::posts(session.app()),
        0,
        "no ack in the whole sequence may surface an error, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );

    // Force a probe — the same disk-fact refresh a real tab switch triggers;
    // its ack must not surface an error either.
    let db = session.app().db.as_ref().unwrap();
    let db_id = rune_db::DocId(session.app().active_doc().doc_db().unwrap().db_id);
    let probe_op = db.store.probe(db_id).expect("enqueue probe");
    let evt = wait_for_db_op(&db.bridge, probe_op);
    assert!(
        matches!(evt, DbEvent::Ok { .. }),
        "probe must not fail: {evt:?}"
    );

    let live_content = session.snapshot().content;
    assert_eq!(live_content, "alphabetadelta");

    // Restart: a brand-new `Store` on the SAME db path must recover
    // EXACTLY this session's live buffer — the definitive proof the
    // durable journal was never truncated behind an underestimated undo
    // target.
    session
        .app_mut()
        .db
        .take()
        .expect("session has a store")
        .shutdown();
    drop(session);

    let (store_b, bridge_b) = restarted_store_at(&db_path, Arc::clone(&vfs));
    let op_id = store_b.load(doc_path).expect("enqueue load b");
    let load_b = match wait_for_db_op(&bridge_b, op_id) {
        DbEvent::Ok {
            result: OpOutcome::Load(r),
            ..
        } => *r,
        other => panic!("expected a Load ack, got {other:?}"),
    };
    store_b.shutdown();

    assert_eq!(
        load_b.recovered.content, live_content,
        "the recovered document must equal the live buffer — never a truncated/corrupted \
         durable journal behind an underestimated undo target"
    );
}
