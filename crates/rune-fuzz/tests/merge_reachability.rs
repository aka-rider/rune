//! A session without a recovery store must refuse merge with visible
//! feedback and can never activate the resolver; the same scenario with a
//! store reaches the resolver. These two tests pin that boundary so a
//! headless session builder cannot silently drop merge coverage again.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rune_core::buffer::Buffer;
use rune_db::{ClockFn, DbEvent, Store, SyncKind};
use rune_tui::app::{self, App};
use rune_tui::db::{Db, DbBridge};
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::merge::MergeState;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs, VfsTestExt};

fn ctrl(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    }
}

fn press_key(app: &mut App, key: KeyInput) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(key), &mut effects);
}

/// With no recovery store wired in, the merge trigger has nothing to
/// compare against: it must refuse every time, surface a warning the user
/// can see, and never let the resolver become active.
#[test]
fn session_without_a_store_refuses_merge_and_stays_inactive() {
    let path = PathBuf::from("/fuzz/doc.md");
    let mem = Arc::new(Mem::new());
    let _ = mem.save_atomic(&path, b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    let mut app = App::new(
        Buffer::new("hello"),
        Some(
            rune_tui::resolved::ResolvedPath::resolve(vfs.as_ref(), std::path::Path::new(&path))
                .expect("the launch path resolves"),
        ),
        vfs,
        None,
    );
    app.clock = Arc::new(rune_tui::pointer::ManualClock::new());
    app.active_doc_mut().focused = true;
    app.frame = Some(rune_tui::app::FrameSize::new(80, 24));
    app.relayout();
    app.sync_view();

    press_key(&mut app, ctrl('m'));

    assert!(
        matches!(app.merge, MergeState::Inactive),
        "with no store the merge state must stay Inactive, got {:?}",
        app.merge
    );
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        Some("no divergence to merge"),
        "the refusal must surface a message so the action is never silently swallowed"
    );
}

/// A clock that never touches the wall clock, so timing can never leak
/// into an assertion.
fn fixed_clock() -> ClockFn {
    Arc::new(|| SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
}

/// Builds the same session shape as the store-less scenario, but wired to
/// a real recovery store, so the only variable between the two tests is
/// whether a store is present.
fn app_with_store(vfs: Arc<dyn Vfs + Send + Sync>) -> (App, Arc<DbBridge>) {
    let bridge = DbBridge::bootstrap();
    let store = Store::open_in_memory(fixed_clock(), Arc::clone(&vfs), bridge.on_event())
        .expect("open in-memory store");
    let db = Db::new(store, Arc::clone(&bridge), false);
    let app = App::new(Buffer::new(""), None, vfs, Some(db));
    (app, bridge)
}

/// Feeds the buffered acknowledgement for the single outstanding operation
/// on the given document through the update loop, exactly as the real
/// runtime does when the store thread replies. The wait itself is the
/// fuzz driver's own drain predicate (`rune_fuzz::driver::wait_for_db_op`)
/// — this test's only addition is picking the op by DOCUMENT rather than
/// oldest-first, and panicking loudly on a failure this scenario never
/// expects to see.
fn drain_one_op_for(app: &mut App, bridge: &DbBridge, doc: DocumentId) {
    let op_id = *app
        .db_ops
        .iter()
        .find(|(_, pending)| pending.doc == doc)
        .expect("one op recorded for this document")
        .0;
    match rune_fuzz::driver::wait_for_db_op(bridge, op_id) {
        evt @ DbEvent::Ok { .. } => {
            let mut effects = Effects::default();
            app::update(app, Msg::Db(evt), &mut effects);
        }
        DbEvent::Err { id, error } => panic!("op {id} failed: {error}"),
        DbEvent::Fatal { error } => panic!("writer thread fatal: {error}"),
    }
}

/// Writes bytes to disk the same way materialize does: a durable temp
/// write followed by an atomic publish.
fn publish(vfs: &dyn Vfs, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

/// Simulates another process changing the file on disk out from under the
/// open document.
fn external_write(vfs: &dyn Vfs, path: &Path, bytes: &[u8]) {
    vfs.remove(path).expect("remove the stale file");
    publish(vfs, path, bytes);
}

/// The same scenario, store-backed: open a file, journal a local edit,
/// diverge the disk copy behind its back, and let a reprobe detect it.
/// The merge trigger must then queue preparation and, once that
/// acknowledgement lands, hand control to an active resolver — proving
/// the refusal above is caused by the missing store and nothing else.
#[test]
fn session_with_a_store_reaches_an_active_resolver_on_divergence() {
    let doc_path = Path::new("/doc.md");
    let mem = Mem::new();
    publish(&mem, doc_path, b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store(Arc::clone(&vfs));
    let draft_id = app.active;
    workspace::open_path(&mut app, doc_path);
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Clean));

    // Ours moves: one journaled edit, acknowledgement drained.
    press_key(
        &mut app,
        KeyInput {
            code: KeyCode::Char('!'),
            mods: Mods::NONE,
        },
    );
    assert_eq!(app.doc(doc_id).unwrap().buffer.content(), "!hello");
    drain_one_op_for(&mut app, &bridge, doc_id);

    // Theirs moves too: divergence is only caught on the away-and-back
    // reprobe, since this phase has no file watcher.
    external_write(vfs.as_ref(), doc_path, b"disk changed this");
    workspace::switch_to(&mut app, draft_id);
    workspace::switch_to(&mut app, doc_id);
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Diverged));

    app.active = doc_id;
    press_key(&mut app, ctrl('m'));
    assert!(
        matches!(app.merge, MergeState::Pending { .. }),
        "the merge trigger on a diverged store-backed document must enqueue preparation, got {:?}",
        app.merge
    );
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert!(
        matches!(app.merge, MergeState::Active { .. }),
        "the preparation acknowledgement must activate the resolver, got {:?}",
        app.merge
    );
}
