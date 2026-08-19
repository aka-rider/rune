use super::*;

/// The property that separates a missing-path launch from an untitled
/// draft: `file_path` stays `Some(the path)`, not `None`. Multi-positional
/// on purpose (a real, existing second file) so this also pins that the
/// `DocDb` lands on the FIRST positional's document — the one bootstrap
/// actually binds a scratch row to — and not on the extra tab that opens
/// through the ordinary async `Load` path.
#[test]
fn launch_missing_first_positional_pins_file_path_and_only_the_first_docs_db() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/vault/other.md"), b"other")
        .expect("seed other.md");
    let home = ScratchHome::new("missing-multi");

    let app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![
            OsString::from("/vault/missing.md"),
            OsString::from("/vault/other.md"),
        ]
        .into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed");

    assert_eq!(app.documents.len(), 2);
    let active = app.doc(app.active).expect("active doc exists");
    assert_eq!(
        active.file_path.as_deref(),
        Some(Path::new("/vault/missing.md")),
        "the recovery-backed missing-path document must keep its intended name, \
         not fall back to an untitled draft"
    );
    assert!(
        active
            .doc_db()
            .is_some_and(|db| db.publish_mode.is_create_only()),
        "the first positional's document must bind the fresh scratch row"
    );

    let other = app
        .documents
        .values()
        .find(|d| d.file_path.as_deref() == Some(Path::new("/vault/other.md")))
        .expect("the second positional opened its own tab");
    assert!(
        !other.is_store_bound(),
        "the DocDb from bootstrap_new_file must land on the first positional's \
         document, never on an extra tab"
    );
}

/// The plan's deliberate rejection of a `gc_empty_scratch` sweep inside
/// `bootstrap_new_file`: a second concurrent launch sharing this `$HOME`
/// must never sweep away another (still-running) session's freshly minted,
/// not-yet-journaled scratch row. Seeds exactly that row directly, without
/// going through `bootstrap` at all, then confirms a missing-path launch
/// leaves it standing.
#[test]
fn launch_missing_first_positional_never_sweeps_another_sessions_empty_scratch_row() {
    let home = ScratchHome::new("missing-path-no-gc");
    let db_path = home
        .0
        .join("Library")
        .join("Application Support")
        .join("rune")
        .join(rune_db::db_file_name(rune_db::SCHEMA_VERSION));

    let other_sessions_row_id = {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let bridge = rune_tui::db::DbBridge::bootstrap();
        let (store, _warning) = rune_db::Store::open(&db_path, Arc::clone(&vfs), bridge.on_event())
            .expect("open store");

        let create_op = store.create_scratch().expect("enqueue create_scratch");
        let db_id = match bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == create_op,
            rune_db::DbEvent::Fatal { .. } => true,
        }) {
            rune_db::DbEvent::Ok {
                result: rune_db::OpOutcome::ScratchDocId(id),
                ..
            } => id.0,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };

        store.shutdown();
        db_id
    };

    let vfs = Mem::new();
    let _app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![OsString::from("/vault/missing.md")].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed for a missing-path launch");

    let raw =
        rune_db::open_raw_connection_at_path_for_test(&db_path).expect("open db file directly");
    let still_present: bool = raw
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            [other_sessions_row_id],
            |r| r.get(0),
        )
        .expect("check the other session's row");
    assert!(
        still_present,
        "bootstrap_new_file must never sweep another session's empty scratch row"
    );
}

#[test]
fn bare_launch_never_sweeps_another_sessions_empty_scratch_row() {
    let home = ScratchHome::new("bare-launch-no-gc");
    let db_path = home
        .0
        .join("Library")
        .join("Application Support")
        .join("rune")
        .join(rune_db::db_file_name(rune_db::SCHEMA_VERSION));

    let other_sessions_row_id = {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let bridge = rune_tui::db::DbBridge::bootstrap();
        let (store, _warning) = rune_db::Store::open(&db_path, Arc::clone(&vfs), bridge.on_event())
            .expect("open store");

        let create_op = store.create_scratch().expect("enqueue create_scratch");
        let db_id = match bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == create_op,
            rune_db::DbEvent::Fatal { .. } => true,
        }) {
            rune_db::DbEvent::Ok {
                result: rune_db::OpOutcome::ScratchDocId(id),
                ..
            } => id.0,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };

        store.shutdown();
        db_id
    };

    let vfs = Mem::new();
    let _app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        std::iter::empty(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed for a no-positional launch");

    let raw =
        rune_db::open_raw_connection_at_path_for_test(&db_path).expect("open db file directly");
    let still_present: bool = raw
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            [other_sessions_row_id],
            |r| r.get(0),
        )
        .expect("check the other session's row");
    assert!(
        still_present,
        "bootstrap_untitled_db must never sweep another still-running session's \
         empty scratch row"
    );
}

/// The scratch row a no-positional launch binds to must actually
/// survive: typing into the default document, then relaunching against
/// the SAME `$HOME` with no positional files again, must come back with
/// that text already in the buffer (crash recovery for the untitled
/// draft, not just a live journal nobody ever reads back).
#[test]
fn a_dead_sessions_untitled_draft_is_recovered_on_the_next_launch() {
    let home = ScratchHome::new("untitled-recover");

    let db_path = home
        .0
        .join("Library")
        .join("Application Support")
        .join("rune")
        .join(rune_db::db_file_name(rune_db::SCHEMA_VERSION));

    // First "session": open the store directly (bypassing the whole app)
    // and journal an edit under it, exactly as typing into the default
    // untitled document would — then drop it without ever naming the
    // document, simulating a crash/quit that left the draft unsaved.
    {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let bridge = rune_tui::db::DbBridge::bootstrap();
        let (store, _warning) = rune_db::Store::open(&db_path, Arc::clone(&vfs), bridge.on_event())
            .expect("open store");

        let create_op = store.create_scratch().expect("enqueue create_scratch");
        let doc_id = match bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == create_op,
            rune_db::DbEvent::Fatal { .. } => true,
        }) {
            rune_db::DbEvent::Ok {
                result: rune_db::OpOutcome::ScratchDocId(id),
                ..
            } => id.0,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };

        // A bare recovery-anchor snapshot is enough on its own: it
        // carries this session's own `session_id`, which is all
        // `most_recent_session_for_doc`/`recover_document` need to find
        // and replay it — no separate journaled edit required.
        let snapshot_op = store
            .create_snapshot(rune_db::DocId(doc_id), "unsaved draft from a dead session")
            .expect("enqueue create_snapshot");
        bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => {
                *id == snapshot_op
            }
            rune_db::DbEvent::Fatal { .. } => true,
        });

        store.shutdown();
    }

    // Both "sessions" above and below run in this SAME test process, so
    // the first session's own pid is trivially still alive by the time
    // the second bootstrap runs — stamp its `sessions` row with an
    // unambiguously-dead pid (`is_process_alive`'s own `pid <= 0` guard)
    // directly through a raw connection, the only way to simulate a
    // truly dead prior process without spawning a real second one.
    {
        let raw =
            rune_db::open_raw_connection_at_path_for_test(&db_path).expect("open db file directly");
        raw.execute("UPDATE sessions SET pid = -1", [])
            .expect("mark every recorded session dead");
    }

    // Second "session": a plain no-positional launch against the SAME
    // `$HOME` — the prior session's process is now unambiguously dead,
    // so `reconstruct_scratch` must find it and recover the draft.
    let vfs = Mem::new();
    let app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        std::iter::empty(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("second bootstrap should succeed");

    assert_eq!(
        app.active_doc().buffer.content(),
        "unsaved draft from a dead session",
        "the dead session's own draft must come back on the next launch"
    );
}

/// Marks every recorded session's `pid` unambiguously dead
/// (`is_process_alive`'s own `pid <= 0` guard) directly through a raw
/// connection — both "sessions" in these tests run in this SAME test
/// process, so the prior one's real pid is trivially still alive by the
/// time the next bootstrap runs; this is the only way to simulate a truly
/// dead prior process without spawning a real second one.
fn mark_every_session_dead(db_path: &Path) {
    let raw =
        rune_db::open_raw_connection_at_path_for_test(db_path).expect("open db file directly");
    raw.execute("UPDATE sessions SET pid = -1", [])
        .expect("mark every recorded session dead");
}

fn db_path_under(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("rune")
        .join(rune_db::db_file_name(rune_db::SCHEMA_VERSION))
}

/// The main Task B recovery pin: `rune notes.md` for a path that does not
/// exist on disk, typed into and killed without ever materializing, must be
/// found and adopted — SAME content, SAME underlying row — by a LATER `rune
/// notes.md` launch, not silently orphaned the way a bare `path=''`-only
/// scratch lookup would leave it (a positional launch never lists among
/// `recoverable_scratch`'s candidates, which is exactly why this needs its
/// own `intended_path`-keyed lookup).
#[test]
fn named_positional_dead_session_draft_is_recovered_on_relaunch_of_the_same_path() {
    let home = ScratchHome::new("named-positional-recover");
    let db_path = db_path_under(&home.0);
    let intended_path = "/vault/notes.md";

    let first_doc_id = {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let bridge = rune_tui::db::DbBridge::bootstrap();
        let (store, _warning) = rune_db::Store::open(&db_path, Arc::clone(&vfs), bridge.on_event())
            .expect("open store");

        let create_op = store
            .create_named_scratch(intended_path)
            .expect("enqueue create_named_scratch");
        let doc_id = match bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == create_op,
            rune_db::DbEvent::Fatal { .. } => true,
        }) {
            rune_db::DbEvent::Ok {
                result: rune_db::OpOutcome::ScratchDocId(id),
                ..
            } => id.0,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };

        let snapshot_op = store
            .create_snapshot(rune_db::DocId(doc_id), "typed before the crash")
            .expect("enqueue create_snapshot");
        bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => {
                *id == snapshot_op
            }
            rune_db::DbEvent::Fatal { .. } => true,
        });

        store.shutdown();
        doc_id
    };

    mark_every_session_dead(&db_path);

    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let app = bootstrap(
        &vfs,
        vec![OsString::from(intended_path)].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("relaunch of the same path should succeed");

    assert_eq!(
        app.active_doc().buffer.content(),
        "typed before the crash",
        "the dead session's own named draft must come back on a relaunch of the same path"
    );
    assert_eq!(
        app.doc_db_id(app.active),
        Some(first_doc_id),
        "the relaunch must bind the SAME recovery row, not a fresh one copying the text in"
    );
}

/// A DIFFERENT positional must never adopt another path's draft — the whole
/// point of keying the lookup on `intended_path` rather than "any unbound
/// scratch with history".
#[test]
fn named_positional_draft_is_not_adopted_by_a_different_positional() {
    let home = ScratchHome::new("named-positional-different-path");
    let db_path = db_path_under(&home.0);

    {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let bridge = rune_tui::db::DbBridge::bootstrap();
        let (store, _warning) = rune_db::Store::open(&db_path, Arc::clone(&vfs), bridge.on_event())
            .expect("open store");

        let create_op = store
            .create_named_scratch("/vault/notes.md")
            .expect("enqueue create_named_scratch");
        let doc_id = match bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == create_op,
            rune_db::DbEvent::Fatal { .. } => true,
        }) {
            rune_db::DbEvent::Ok {
                result: rune_db::OpOutcome::ScratchDocId(id),
                ..
            } => id,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };

        let snapshot_op = store
            .create_snapshot(doc_id, "typed before the crash")
            .expect("enqueue create_snapshot");
        bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => {
                *id == snapshot_op
            }
            rune_db::DbEvent::Fatal { .. } => true,
        });

        store.shutdown();
    }

    mark_every_session_dead(&db_path);

    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let app = bootstrap(
        &vfs,
        vec![OsString::from("/vault/other.md")].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("relaunch of a different path should succeed");

    assert_eq!(
        app.active_doc().buffer.content(),
        "",
        "a differently-named positional must never adopt another path's draft"
    );
}

/// A still-alive session's own named draft must never be stolen by a
/// concurrent launch of the same positional — the multi-instance safety
/// requirement: `reconstruct_scratch`'s liveness check (the same one
/// `bootstrap_untitled_db`'s bare-launch recovery already relies on) must
/// refuse it exactly as it would for the untitled case.
#[test]
fn named_positional_draft_of_a_live_session_is_not_stolen() {
    let home = ScratchHome::new("named-positional-live-session");
    let db_path = db_path_under(&home.0);
    let intended_path = "/vault/notes.md";

    // First "session" stays open (never shut down) so its `sessions` row
    // keeps this test process's own, still-very-much-alive pid — no
    // `mark_every_session_dead` call on this test.
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let bridge = rune_tui::db::DbBridge::bootstrap();
    let (store, _warning) =
        rune_db::Store::open(&db_path, Arc::clone(&vfs), bridge.on_event()).expect("open store");

    let create_op = store
        .create_named_scratch(intended_path)
        .expect("enqueue create_named_scratch");
    let first_doc_id = match bridge.wait_for_bootstrap_event(|evt| match evt {
        rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == create_op,
        rune_db::DbEvent::Fatal { .. } => true,
    }) {
        rune_db::DbEvent::Ok {
            result: rune_db::OpOutcome::ScratchDocId(id),
            ..
        } => id.0,
        other => panic!("expected a CreateScratch ack, got {other:?}"),
    };

    let snapshot_op = store
        .create_snapshot(rune_db::DocId(first_doc_id), "still being typed")
        .expect("enqueue create_snapshot");
    bridge.wait_for_bootstrap_event(|evt| match evt {
        rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == snapshot_op,
        rune_db::DbEvent::Fatal { .. } => true,
    });

    let vfs2: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let app = bootstrap(
        &vfs2,
        vec![OsString::from(intended_path)].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("a concurrent launch of the same path should still succeed");

    assert_eq!(
        app.active_doc().buffer.content(),
        "",
        "a live session's own named draft must never be handed to a concurrent launch"
    );
    assert_ne!(
        app.doc_db_id(app.active),
        Some(first_doc_id),
        "a concurrent launch must mint its own row, never bind the live session's row"
    );

    store.shutdown();
}
