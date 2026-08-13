//! `local_seq` desync regression suite: an `AppendEdit` replica skipped
//! pre-bind. Driven through `rune_fuzz::Session`; real-`Store` fixtures
//! come from `db_wiring_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;
use std::sync::Arc;

use rune_fuzz::Session;
use rune_tui::db::Db;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use db_wiring_common::{publish, restarted_store_at, store_at, temp_db_dir};

const END: KeyInput = KeyInput {
    code: KeyCode::End,
    mods: Mods::NONE,
};

const UNDO: KeyInput = KeyInput {
    code: KeyCode::Char('z'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};

const REDO: KeyInput = KeyInput {
    code: KeyCode::Char('z'),
    mods: Mods {
        shift: true,
        alt: false,
        ctrl: false,
        sup: true,
    },
};

/// The core repro: a document opened through the store (a `Load`
/// in flight) receives several keystrokes BEFORE that `Load`'s own ack
/// lands. Every one of those pre-bind edits must still reach the durable
/// journal once the ack installs the document's `DocDb` — restoring the 1:1
/// correspondence between local journal positions and durable `events`
/// rows an unconditionally-skipped `AppendEdit` used to break — and undo
/// must still work cleanly afterward.
#[test]
fn prebind_edits_replay_at_bind() {
    let mut session = Session::open("/seed.md", "seed");
    publish(session.app().vfs.as_ref(), Path::new("/doc.md"), b"hello");

    let id = workspace::open_path(session.app_mut(), Path::new("/doc.md")).expect("open doc");
    session.app_mut().active_doc_mut().focused = true;

    // Three keystrokes land while the Load round trip is still in flight —
    // nothing else may enqueue against the store meanwhile, so `db_ops`
    // must still hold only the one Load op.
    assert!(session.key(END).is_none());
    assert!(session.type_("abc").is_none());
    assert_eq!(session.app().doc(id).unwrap().buffer.content(), "helloabc");
    assert_eq!(
        session.app().db_ops.len(),
        1,
        "pre-bind edits must never enqueue their own AppendEdit while Binding"
    );
    assert!(
        !session.app().doc(id).unwrap().is_store_bound(),
        "the document is not yet Bound while its Load is still in flight"
    );

    // The Load ack lands — content on disk never diverged from what this
    // session read, so hydration is a plain NoChange; no bridge Step
    // pushed, `undo_base` stays 0.
    assert!(session.deliver_db().is_none());

    assert!(
        session.app().doc(id).unwrap().is_store_bound(),
        "the Load ack must install DocDb"
    );
    assert_eq!(session.app().doc(id).unwrap().buffer.content(), "helloabc");
    assert_eq!(
        session.app().doc(id).unwrap().journal.len(),
        3,
        "three local journal positions: one per pre-bind keystroke"
    );
    assert_eq!(
        session.app().db_ops.len(),
        3,
        "all three pre-bind edits must replay as real AppendEdit enqueues"
    );

    assert!(session.deliver_db_all().is_none());
    assert!(
        session.app().db_ops.is_empty(),
        "every replayed op must be acked"
    );
    assert!(
        !session.app().db.as_ref().unwrap().degraded,
        "the store must stay healthy through the whole replay"
    );

    // Undo must resolve cleanly against the now-durable journal — the exact
    // failure mode a dropped pre-bind edit used to produce when it was
    // silently skipped instead of replayed: the writer thread's own
    // local-position count would then be short by however many edits never
    // reached it, and MoveUndoPos would resolve to the wrong durable seq
    // (or fail outright once the desync ran past the end of `local_seq`).
    assert!(session.key(UNDO).is_none());
    assert!(session.deliver_db().is_none());
    assert_eq!(session.app().doc(id).unwrap().buffer.content(), "helloab");
    assert!(
        !session.app().db.as_ref().unwrap().degraded,
        "an undo resolving cleanly must never degrade the store"
    );
}

/// An adopting hydration (a dead session's own unsaved draft, recovered and
/// bridged onto disk content by the store itself) pushes a synthetic bridge
/// `Step` directly onto the local journal — permanently offsetting it by
/// one position relative to the writer thread's own local-position count,
/// which never sees that bridge as an `AppendEdit`. `DocDb::undo_base`
/// exists to correct exactly this offset; undo/redo must resolve cleanly
/// all the way back through the bridge to the pre-crash disk anchor, with
/// no error and no store degrade.
#[test]
fn undo_after_adoption_resolves() {
    let dir = temp_db_dir("prebind-adoption");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    // Session A: types more, never saves (materializes) to disk, then
    // "crashes" (its own journal stays durable; the process just vanishes).
    let (store_a, bridge_a) = store_at(&db_path, Arc::clone(&vfs));
    let mut session_a = Session::open_with_db(
        "/doc.md",
        Arc::clone(&mem),
        Db::new(store_a, bridge_a, false),
    );
    assert!(session_a.key(END).is_none());
    assert!(session_a.type_(" world").is_none());
    assert_eq!(session_a.snapshot().content, "hello world");
    assert!(session_a.deliver_db_all().is_none());
    session_a
        .app_mut()
        .db
        .take()
        .expect("session A has a store")
        .shutdown();
    drop(session_a);

    // Session B ("restart"): a brand-new `Store` on the SAME path, with
    // session A reported dead, hydrating through the ordinary ASYNC
    // open/ack path exactly like a real restart — `db_ack::handle_load_ack`
    // is what sets `undo_base` here, not test scaffolding.
    let (store_b, bridge_b) = restarted_store_at(&db_path, Arc::clone(&vfs));
    let mut session_b = Session::open_with_db(
        "/doc.md",
        Arc::clone(&mem),
        Db::new(store_b, bridge_b, false),
    );

    assert_eq!(
        session_b.snapshot().content,
        "hello world",
        "the adopting Load must recover session A's unsaved edit"
    );
    assert!(session_b.app().active_doc().is_store_bound());

    // One more edit after adoption, then undo back through it (removing
    // the typed edit), and once more (through the undo_base-corrected
    // bridge, back to the pre-crash disk anchor), then redo twice back to
    // where undo started — no error, no degrade, at every step.
    assert!(session_b.key(END).is_none());
    assert!(session_b.type_("!").is_none());
    assert_eq!(session_b.snapshot().content, "hello world!");
    assert!(session_b.deliver_db_all().is_none());

    for expected in ["hello world", "hello"] {
        assert!(session_b.key(UNDO).is_none());
        assert!(session_b.deliver_db().is_none());
        assert_eq!(session_b.snapshot().content, expected);
        assert!(!session_b.app().db.as_ref().unwrap().degraded);
    }

    for expected in ["hello world", "hello world!"] {
        assert!(session_b.key(REDO).is_none());
        assert!(session_b.deliver_db().is_none());
        assert_eq!(session_b.snapshot().content, expected);
        assert!(!session_b.app().db.as_ref().unwrap().degraded);
    }

    assert!(
        rune_tui::messages::newest_text(session_b.app())
            .is_none_or(|m| !m.contains("undo failed") && !m.contains("redo failed")),
        "no undo/redo error may ever have been posted"
    );
}

/// A `MoveUndoPos` resolution failure is a fact about ONE document's own
/// local-position bookkeeping, never evidence the whole store can no
/// longer be trusted — it must surface as a per-document error and leave
/// `Db::degraded` false, unlike an `AppendEdit`/`Load` failure.
#[test]
fn undo_pos_error_is_doc_scoped() {
    let mut session = Session::open("/doc.md", "hello");
    let id = session.app().active;
    assert!(session.app().doc(id).unwrap().is_store_bound());

    // A local position no `AppendEdit` this session has ever run could
    // possibly resolve to — the writer thread's own `MoveUndoPos` handler
    // must refuse it as `Error::NotFound`.
    rune_tui::db_enqueue::move_undo_pos(session.app_mut(), id, 999);
    assert!(session.deliver_db().is_none());

    assert!(
        !session.app().db.as_ref().unwrap().degraded,
        "a MoveUndoPos failure must never degrade the whole store"
    );
    assert!(
        rune_tui::messages::newest_text(session.app()).is_some_and(|m| m.contains("doc.md")),
        "the failure must surface as a per-document error naming the document"
    );
}

/// While `Detached` (here: a degraded store at open time), every edit is a
/// plain no-op for the replica — no `AppendEdit`/`Load` ever enqueues, and
/// there is no `Binding` window buffering anything to leak.
#[test]
fn detached_document_buffers_nothing() {
    let dir = temp_db_dir("prebind-detached");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    let (store, bridge) = store_at(&dir.join("rune-v1.db"), vfs);
    let mut session =
        Session::open_with_db("/doc.md", Arc::clone(&mem), Db::new(store, bridge, true));

    assert!(
        session.app().db_ops.is_empty(),
        "a degraded store must never enqueue a Load"
    );
    assert!(!session.app().active_doc().is_store_bound());

    assert!(session.key(END).is_none());
    assert!(session.type_("xyz").is_none());
    assert_eq!(session.snapshot().content, "helloxyz");
    assert!(
        session.app().db_ops.is_empty(),
        "Detached must never enqueue an AppendEdit, and never buffer one to replay later"
    );
    assert!(!session.app().active_doc().is_store_bound());
}
