//! Integration tests for the rune-tui <-> rune-db
//! wiring's hydration paths: post-restart hydration/undo and `Load`-ack
//! adoption into `Document`/`DocDb`, driven through `rune_fuzz::Session`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{DbEvent, LoadResult, OpOutcome, SyncKind, SyncState, Version};
use rune_fuzz::Session;
use rune_tui::app::{self, App};
use rune_tui::db::{Db, LoadPurpose, PendingOp};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
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

const SELECT_ALL: KeyInput = KeyInput {
    code: KeyCode::Char('a'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};

const BACKSPACE: KeyInput = KeyInput {
    code: KeyCode::Backspace,
    mods: Mods::NONE,
};

/// Edits journaled by one session -> a NEW `Store`
/// opened on the SAME db path (a simulated restart) hydrates the recovered
/// content through the real `Load`-ack path, and undo reaches the pre-crash
/// anchor.
#[test]
fn restart_hydrates_content_and_undo_reaches_the_anchor() {
    let dir = temp_db_dir("restart");
    let db_path = dir.join("rune-v1.db");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    // Session A: types more, never saves (materializes) to disk.
    let (store_a, bridge_a) = store_at(&db_path, Arc::clone(&vfs));
    let mut session_a = Session::open_with_db(
        "/doc.md",
        Arc::clone(&mem),
        Db::new(store_a, bridge_a, false),
    );
    assert!(session_a.key(END).is_none());
    assert!(session_a.type_(" world").is_none());
    assert_eq!(session_a.snapshot().content, "hello world");
    assert!(
        session_a.app().db_banner.is_none(),
        "session A's own store must stay healthy throughout"
    );
    assert!(session_a.deliver_db_all().is_none());

    // Every journaled edit must be durably committed before "restarting" —
    // `Store::shutdown` drains its writer FIFO to empty before returning
    // (deterministic; no polling needed).
    session_a
        .app_mut()
        .db
        .take()
        .expect("session A has a store")
        .shutdown();
    drop(session_a);

    // Session B (simulated restart): a brand-new `Store` on the SAME path,
    // with session A reported dead, hydrating through the ordinary
    // open/ack path — `db_ack::handle_load_ack` seeds the bridge step and
    // the undo mapping here, not test scaffolding.
    let (store_b, bridge_b) = restarted_store_at(&db_path, Arc::clone(&vfs));
    let mut session_b = Session::open_with_db(
        "/doc.md",
        Arc::clone(&mem),
        Db::new(store_b, bridge_b, false),
    );

    assert_eq!(
        session_b.snapshot().content,
        "hello world",
        "restart must recover session A's unsaved edits"
    );
    assert_eq!(
        mem.read(Path::new("/doc.md")).expect("file still readable"),
        b"hello",
        "the on-disk file itself was never touched — session A never saved"
    );

    assert!(session_b.key(UNDO).is_none());
    assert!(session_b.deliver_db().is_none());
    assert_eq!(
        session_b.snapshot().content,
        "hello",
        "post-restart undo must reach the pre-crash anchor (the disk content)"
    );
}

/// The `Load` ack installs `Document::db` as `Some` once it lands.
#[test]
fn load_ack_installs_document_db_as_some() {
    let mut session = Session::open("/seed.md", "seed");
    publish(session.app().vfs.as_ref(), Path::new("/doc.md"), b"hello");

    let id = workspace::open_path(session.app_mut(), Path::new("/doc.md")).expect("open doc");
    assert!(
        !session.app().doc(id).unwrap().is_store_bound(),
        "db stays None until the Load ack lands"
    );

    assert!(session.deliver_db().is_none());

    assert!(
        session.app().doc(id).unwrap().is_store_bound(),
        "a Load ack with a saved_obs baseline must install DocDb"
    );
    assert!(
        session.app().db_ops.is_empty(),
        "the ack must pop its own db_ops entry"
    );
    assert_eq!(
        session.app().doc(id).unwrap().buffer.content(),
        "hello",
        "no divergence to recover: the buffer stays exactly what was read off disk"
    );
}

/// Data-safety guard: an ack for a document the user kept
/// typing into during the async round trip must NEVER clobber those
/// keystrokes — the buffer bytes stay exactly as typed, even though the
/// ack's own `recovered` content would otherwise differ from what's now on
/// screen. `DocDb` is still installed: the document's own recovery journal
/// is real and should be used going forward.
#[test]
fn ack_for_a_document_edited_during_the_round_trip_leaves_the_buffer_unchanged() {
    let mut session = Session::open("/seed.md", "seed");
    publish(session.app().vfs.as_ref(), Path::new("/doc.md"), b"hello");

    let id = workspace::open_path(session.app_mut(), Path::new("/doc.md")).expect("open doc");
    session.app_mut().active_doc_mut().focused = true;

    // The user types while the Load round trip is still in flight — this
    // bumps the buffer's version past what was recorded at enqueue time.
    assert!(session.key(END).is_none());
    assert!(session.type_("!").is_none());
    assert_eq!(session.app().doc(id).unwrap().buffer.content(), "hello!");

    assert!(session.deliver_db().is_none());

    assert_eq!(
        session.app().doc(id).unwrap().buffer.content(),
        "hello!",
        "the ack must never clobber a keystroke typed during the round trip"
    );
    assert!(
        session.app().doc(id).unwrap().is_store_bound(),
        "DocDb must still be installed even when the buffer adopt is skipped"
    );
}

/// A `Load` ack whose `LoadResult` carries no `saved_obs` baseline (should
/// not occur in practice — see `LoadResult::saved_obs`'s own doc comment;
/// exercised here directly since a real `Store::load` always adopts one on
/// a first load) must install nothing and surface a status message rather
/// than binding a document to a recovery row with no CAS baseline.
#[test]
fn ack_with_no_saved_obs_leaves_db_none_and_posts_a_message() {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let mut app = App::new(
        Buffer::new("hello"),
        Some(PathBuf::from("/doc.md")),
        vfs,
        None,
    );
    let id = app.active;

    let op_id = 1u64;
    let issued_version = app.doc(id).unwrap().buffer.version();
    app.db_ops.insert(
        op_id,
        PendingOp::load(id, issued_version, LoadPurpose::Recover),
    );

    let load_result = LoadResult {
        doc_id: rune_db::DocId(1),
        renamed_from: None,
        disk_content: "hello".to_string(),
        recovered: "hello".to_string(),
        has_history: false,
        sync: SyncState {
            kind: SyncKind::Clean,
            ancestor: None,
            ours: Version {
                hash: rune_db::BlobHash(String::new()),
                obs: None,
            },
            theirs: None,
        },
        nlink: 1,
        saved_obs: None,
        bridge_seq: None,
        resumable_merge: None,
    };

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Db(DbEvent::Ok {
            id: op_id,
            result: OpOutcome::Load(Box::new(load_result)),
        }),
        &mut effects,
    );

    assert!(
        !app.doc(id).unwrap().is_store_bound(),
        "no baseline observation means no DocDb binding"
    );
    assert_eq!(app.doc(id).unwrap().buffer.content(), "hello");
    assert!(
        rune_tui::messages::newest_text(&app)
            .is_some_and(|s| s.contains("no baseline observation")),
        "a status message must explain why crash recovery wasn't bound (got {:?})",
        rune_tui::messages::newest_text(&app)
    );
}

/// `handle_load_ack` must refuse
/// to adopt recovered content that would empty (or drastically shrink) a
/// non-empty on-disk file — the destructive-async-reset suspicion check,
/// run through the shared `Document::hydrate` chokepoint. Reached through a
/// REAL round trip: session A deletes everything and dies unsaved, so the
/// restarted session's `Load` genuinely recovers an empty draft against a
/// non-empty disk file. The buffer stays exactly what was on disk, and a
/// status message explains why.
#[test]
fn ack_refuses_to_adopt_recovered_content_that_would_empty_the_disk_content() {
    let dir = temp_db_dir("refuse-empty-adopt");
    let db_path = dir.join("rune-v1.db");
    let disk_content = "a whole paragraph of real content that must not vanish";
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), disk_content.as_bytes());
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    // Session A: deletes the whole document, never saves, then "dies".
    let (store_a, bridge_a) = store_at(&db_path, Arc::clone(&vfs));
    let mut session_a = Session::open_with_db(
        "/doc.md",
        Arc::clone(&mem),
        Db::new(store_a, bridge_a, false),
    );
    assert!(session_a.key(SELECT_ALL).is_none());
    assert!(session_a.key(BACKSPACE).is_none());
    assert_eq!(session_a.snapshot().content, "");
    assert!(session_a.deliver_db_all().is_none());
    session_a
        .app_mut()
        .db
        .take()
        .expect("session A has a store")
        .shutdown();
    drop(session_a);

    // Session B (restart): the recovered empty draft is exactly the
    // destructive async-reset pattern that must never be adopted silently.
    let (store_b, bridge_b) = restarted_store_at(&db_path, Arc::clone(&vfs));
    let session_b = Session::open_with_db(
        "/doc.md",
        Arc::clone(&mem),
        Db::new(store_b, bridge_b, false),
    );

    let app = session_b.app();
    let id = app.active;
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        disk_content,
        "a refused hydration must leave the buffer exactly as it was on disk"
    );
    assert!(
        !app.doc(id).unwrap().is_dirty(),
        "a refused hydration must not mark the buffer dirty"
    );
    assert!(
        !app.doc(id).unwrap().is_store_bound(),
        "a document whose recovered content this session just \
         rejected must never keep journaling against that row"
    );
    assert!(
        rune_tui::messages::newest_text(app).is_some_and(|s| s.contains("crash recovery")),
        "a status message must explain the refusal (got {:?})",
        rune_tui::messages::newest_text(app)
    );
}
