use super::*;

/// An unconfirmed read whose hash happens to match the baseline decides
/// nothing: a file whose bracket can never settle refuses the
/// compare-and-swap as a conflict instead of trusting a hash-equal
/// snapshot it could not stabilize.
#[test]
fn an_unconfirmed_hash_equal_disk_refuses_the_save_as_a_conflict() {
    let path = Path::new("/doc.md");
    let inner = Mem::new();
    publish(&inner, path, b"the baseline");
    let vfs = FlappingStatVfs {
        inner,
        calls: AtomicUsize::new(0),
    };
    let expect_hash = rune_db::hash_bytes(b"the baseline");

    let outcome = run_materialize_vfs(
        &vfs,
        path,
        PublishMode::OverwriteExisting,
        "new content",
        &expect_hash,
        None,
        SaveMode::Normal,
    );

    match outcome {
        MaterializeVfsOutcome::Conflict { confirmed, .. } => {
            assert!(!confirmed, "a churning bracket must never confirm");
        }
        other => panic!("expected a conflict refusal, got {other:?}"),
    }
    assert_eq!(
        vfs.read(path).unwrap(),
        b"the baseline",
        "a refused save must leave the destination untouched"
    );
}

/// A commit whose post-publish stat bracket never settles (a racer in the
/// publish-to-stat gap) must never masquerade as a stable observation:
/// the outcome still commits, but `confirmed: false`.
#[test]
fn a_flapping_post_publish_stat_commits_unconfirmed() {
    let path = Path::new("/doc.md");
    let inner = Mem::new();
    publish(&inner, path, b"the baseline");
    let vfs = FlappingStatVfs {
        inner,
        calls: AtomicUsize::new(0),
    };
    let expect_hash = rune_db::hash_bytes(b"the baseline");

    let outcome = run_materialize_vfs(
        &vfs,
        path,
        PublishMode::OverwriteExisting,
        "new content",
        &expect_hash,
        None,
        SaveMode::Force,
    );

    match outcome {
        MaterializeVfsOutcome::Committed { confirmed, .. } => {
            assert!(!confirmed, "a churning post-publish stat must not confirm");
        }
        other => panic!("expected a committed save, got {other:?}"),
    }
    assert_eq!(vfs.read(path).unwrap(), b"new content");
}

/// A publish whose durability confirmation failed is physical success:
/// reported committed, `durable: false` riding along so the ack side can
/// warn instead of failing the save.
#[test]
fn a_post_publish_durability_failure_still_commits_with_durable_false() {
    let path = Path::new("/doc.md");
    let vfs = Mem::new();
    publish(&vfs, path, b"original");
    vfs.fail_after(rune_vfs::OpKind::Exchange, io::ErrorKind::Other);
    let expect_hash = rune_db::hash_bytes(b"original");

    let outcome = run_materialize_vfs(
        &vfs,
        path,
        PublishMode::OverwriteExisting,
        "new content",
        &expect_hash,
        None,
        SaveMode::Normal,
    );

    match outcome {
        MaterializeVfsOutcome::Committed { durable, .. } => {
            assert!(!durable, "the durability confirmation failed");
        }
        other => panic!("expected a committed save, got {other:?}"),
    }
    assert_eq!(
        vfs.read(path).unwrap(),
        b"new content",
        "the publish itself already took effect"
    );
}

/// Two documents bound to one file saving back-to-back: the second publish
/// must never trip over the first one's temp residue.
#[test]
fn two_saves_of_one_file_back_to_back_never_collide_on_temp_names() {
    let path = Path::new("/doc.md");
    let vfs = Mem::new();
    publish(&vfs, path, b"original");

    let first = run_materialize_vfs(
        &vfs,
        path,
        PublishMode::OverwriteExisting,
        "first tab's content",
        &rune_db::hash_bytes(b"original"),
        None,
        SaveMode::Normal,
    );
    assert!(
        matches!(first, MaterializeVfsOutcome::Committed { .. }),
        "first save must commit, got {first:?}"
    );

    let second = run_materialize_vfs(
        &vfs,
        path,
        PublishMode::OverwriteExisting,
        "second tab's content",
        &rune_db::hash_bytes(b"first tab's content"),
        None,
        SaveMode::Normal,
    );
    assert!(
        matches!(second, MaterializeVfsOutcome::Committed { .. }),
        "second save must commit, got {second:?}"
    );
    assert_eq!(vfs.read(path).unwrap(), b"second tab's content");
}

fn app_with_db() -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: rune_db::ClockFn = Arc::new(std::time::SystemTime::now);
    let store = rune_db::Store::open_in_memory(clock, Arc::clone(&vfs), Box::new(|_evt| {}))
        .expect("open in-memory store");
    let bridge = crate::db::DbBridge::bootstrap();
    let mut app = App::new(rune_core::buffer::Buffer::new("hello"), None, vfs, None);
    app.frame_width = 80;
    app.frame_height = 24;
    app.db = Some(crate::db::Db::new(store, bridge, false));
    let id = app.active;
    if let Some(doc) = app.doc_mut(id) {
        doc.replica = crate::document::Replica::Bound(crate::db::DocDb::new(
            1,
            PublishMode::OverwriteExisting,
            rune_db::Seq(0),
        ));
    }
    app
}

fn type_char(app: &mut App, effects: &mut Effects, c: char) {
    let key = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char(c),
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(app, crate::runtime::Msg::Key(key), effects);
}

/// A journal-mutating keystroke bumps `snapshot_generation` and arms
/// `App::snapshot_timer` (`App::update`'s own wrapper), but the timer
/// itself now owns its deadline's time domain (fix for the debounce
/// decoupling under a `ManualClock`) — this test instead covers the
/// production reaction to the timer's eventual message, delivered exactly
/// as the timer thread would send it: `Msg::SnapshotDue` carrying a
/// generation. A CURRENT generation must enqueue the snapshot; a STALE one
/// (superseded by a later edit) must be silently ignored.
#[test]
fn snapshot_due_with_the_current_generation_enqueues_a_snapshot() {
    let mut app = app_with_db();
    let id = app.active;
    let mut effects = Effects::default();

    type_char(&mut app, &mut effects, 'x');
    let generation = app
        .doc(id)
        .and_then(|d| d.doc_db())
        .expect("db-bound doc")
        .snapshot_generation;
    assert_eq!(generation, 1, "one journal-mutating keystroke bumps once");
    let ops_before = app.db_ops.len();

    crate::app::update(
        &mut app,
        crate::runtime::Msg::SnapshotDue { id, generation },
        &mut effects,
    );

    assert_eq!(
        app.db_ops.len(),
        ops_before + 1,
        "a snapshot for the current generation must be enqueued"
    );
}

#[test]
fn snapshot_due_with_a_stale_generation_is_ignored() {
    let mut app = app_with_db();
    let id = app.active;
    let mut effects = Effects::default();

    type_char(&mut app, &mut effects, 'x');
    let stale_generation = app
        .doc(id)
        .and_then(|d| d.doc_db())
        .expect("db-bound doc")
        .snapshot_generation;
    type_char(&mut app, &mut effects, 'y');
    let ops_before = app.db_ops.len();

    crate::app::update(
        &mut app,
        crate::runtime::Msg::SnapshotDue {
            id,
            generation: stale_generation,
        },
        &mut effects,
    );

    assert_eq!(
        app.db_ops.len(),
        ops_before,
        "a stale generation must never enqueue a snapshot"
    );
}
