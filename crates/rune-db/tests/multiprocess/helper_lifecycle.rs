use super::*;

/// Role (e): [rune-db 8]'s coverage gap — `sweep_unreferenced_blobs` had
/// never been exercised under real cross-process contention. This role
/// repeatedly orphans a blob via the SAME mechanism
/// `journal::new_edit_after_undo_truncates_the_abandoned_future` proves
/// in-process: snapshot a piece of content, undo back to the start,
/// then commit a DIVERGENT edit — the truncation deletes the snapshot
/// row anchored past the new position, orphaning the blob it referenced
/// (`snapshot.rs`'s module doc: journal truncation "deletes both
/// `events` and `snapshots` rows, but the blob a surviving snapshot
/// still points to is untouched" — an orphaned one is fair game for the
/// sibling `gc_sweeper` process racing this one). Every op's ack is
/// asserted `Ok` — the actual claim under test is that concurrent
/// sweeping from another process never causes a legitimate write here
/// to fail or corrupt.
pub(crate) fn gc_editor() {
    let path = db_path();
    let doc_id = rune_db::DocId(env_var("RUNE_DB_DOC_ID").parse().expect("doc id"));
    let count: usize = env_var("RUNE_DB_COUNT").parse().expect("count");
    let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
    let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let store = open_store(&path, on_event);
    let token = rune_db::BindingToken::next();

    touch(&ready);
    wait_for_path(&go, MARKER_SAFETY_DEADLINE);

    for i in 0..count {
        let content_a = format!("round-{i}-a");
        let insert_a = AppliedEdit {
            start: 0,
            end: 0,
            deleted: String::new(),
            insert: content_a.clone(),
        };
        let id = store
            .append_edit(
                doc_id,
                token,
                rune_db::Seq(0),
                EditBatch {
                    edits: &[insert_a],
                    cursors_before: &[],
                    cursors_after: &[],
                    kind: EditKind::Other,
                },
            )
            .expect("enqueue append a");
        recv_seq(&rx, id);

        let id = store
            .create_snapshot(doc_id, &content_a)
            .expect("enqueue snapshot");
        expect_ok(&rx, id);

        let id = store
            .move_undo_pos(doc_id, token, rune_db::Seq(0), 0)
            .expect("enqueue move_undo_pos");
        expect_ok(&rx, id);

        // Diverges from `content_a` — truncates the now-abandoned
        // future, including the snapshot just created, orphaning its
        // blob for the sibling sweeper to find.
        let insert_b = AppliedEdit {
            start: 0,
            end: 0,
            deleted: String::new(),
            insert: format!("round-{i}-b"),
        };
        let id = store
            .append_edit(
                doc_id,
                token,
                rune_db::Seq(0),
                EditBatch {
                    edits: &[insert_b],
                    cursors_before: &[],
                    cursors_after: &[],
                    kind: EditKind::Other,
                },
            )
            .expect("enqueue append b");
        expect_ok(&rx, id);
    }

    store.shutdown();
    std::process::exit(0);
}

/// Role (g): opens `RUNE_DB_DOC_PATH` (a REAL file), journals one unsaved
/// edit on top of it, writes the resulting `doc_id` to
/// `RUNE_DB_DOC_ID_MARKER`, then exits WITHOUT calling `store.shutdown()` —
/// the abrupt, store-preserved quit (`^C^C`) the data-loss regression
/// starts from. The edit's own `append_edit` ack already committed
/// synchronously (WAL), so skipping shutdown loses nothing durable.
pub(crate) fn edit_and_die() {
    let path = db_path();
    let doc_path = PathBuf::from(env_var("RUNE_DB_DOC_PATH"));
    let doc_id_marker = PathBuf::from(env_var("RUNE_DB_DOC_ID_MARKER"));

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let store = open_store(&path, on_event);

    let id = store.load(&doc_path).expect("enqueue load");
    let doc_id = match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::Load(result),
        }) if got == id => result.doc_id,
        other => panic!("expected load ack, got {other:?}"),
    };

    let edit = AppliedEdit {
        start: 0,
        end: 0,
        deleted: String::new(),
        insert: "UNSAVED ".to_string(),
    };
    let id = store
        .append_edit(
            doc_id,
            rune_db::BindingToken::next(),
            rune_db::Seq(0),
            EditBatch {
                edits: &[edit],
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("enqueue append");
    expect_ok(&rx, id);

    std::fs::write(&doc_id_marker, doc_id.to_string()).expect("write doc id marker");
    std::process::exit(0);
}

/// Role (h): reopens `RUNE_DB_DOC_PATH` via a FRESH `Store`/session (the
/// next process), writes the resulting `LoadResult::recovered` content and
/// `sync.kind` to their own marker files for the parent to assert on. The
/// data-loss regression's actual claim under test.
pub(crate) fn reload_diverged() {
    let path = db_path();
    let doc_path = PathBuf::from(env_var("RUNE_DB_DOC_PATH"));
    let recovered_marker = PathBuf::from(env_var("RUNE_DB_RECOVERED_MARKER"));
    let sync_marker = PathBuf::from(env_var("RUNE_DB_SYNC_MARKER"));

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let store = open_store(&path, on_event);

    let id = store.load(&doc_path).expect("enqueue load");
    let load_result = match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::Load(result),
        }) if got == id => *result,
        other => panic!("expected load ack, got {other:?}"),
    };

    std::fs::write(&recovered_marker, &load_result.recovered.content)
        .expect("write recovered marker");
    std::fs::write(&sync_marker, format!("{:?}", load_result.sync.kind))
        .expect("write sync marker");

    store.shutdown();
    std::process::exit(0);
}

pub(crate) fn save_and_die() {
    use rune_vfs::{Etag, PutCondition, PutOutcome, Vfs};

    let path = db_path();
    let doc_path = PathBuf::from(env_var("RUNE_DB_DOC_PATH"));
    let doc_id_marker = PathBuf::from(env_var("RUNE_DB_DOC_ID_MARKER"));
    let insert = env_var("RUNE_DB_INSERT");

    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let store = open_store(&path, on_event);

    let id = store.load(&doc_path).expect("enqueue load");
    let load_result = match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::Load(result),
        }) if got == id => *result,
        other => panic!("expected load ack, got {other:?}"),
    };
    let doc_id = load_result.doc_id;
    let expect = load_result
        .saved_obs
        .expect("a fresh load must adopt a save-CAS baseline");

    let edit = AppliedEdit {
        start: 0,
        end: 0,
        deleted: String::new(),
        insert: insert.clone(),
    };
    let id = store
        .append_edit(
            doc_id,
            rune_db::BindingToken::next(),
            rune_db::Seq(0),
            EditBatch {
                edits: &[edit],
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("enqueue append");
    let seq = match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::Seq(seq),
        }) if got == id => seq,
        other => panic!("expected append ack, got {other:?}"),
    };

    let id = store
        .materialize_prepare(
            doc_id,
            rune_db::MaterializeTarget::Existing { expect },
            None,
        )
        .expect("enqueue materialize prepare");
    let prep = match rx.recv_timeout(MARKER_SAFETY_DEADLINE) {
        Ok(DbEvent::Ok {
            id: got,
            result: rune_db::OpOutcome::MaterializePrep(prep),
        }) if got == id => *prep,
        other => panic!("expected materialize-prepare ack, got {other:?}"),
    };
    let rune_db::MaterializePrep::Overwrite { expect_hash, .. } = prep else {
        panic!("expected an Overwrite prep for an already-loaded document");
    };

    let content = format!("{insert}{}", load_result.disk_content);
    let resolved = rune_vfs::Disk.resolve(&doc_path).expect("resolve doc path");
    let etag = Etag::from_stored(expect_hash.as_str()).expect("parse expect etag");
    let outcome = rune_vfs::put(
        &rune_vfs::Disk,
        &resolved,
        content.as_bytes(),
        PutCondition::IfMatch(etag),
    )
    .expect("publish this session's save");
    let PutOutcome::Committed { sighted, .. } = outcome else {
        panic!("expected a clean commit against a freshly-loaded baseline, got {outcome:?}");
    };

    let id = store
        .materialize_record(
            doc_id,
            &resolved,
            seq.0,
            rune_db::MaterializeOutcome::Committed {
                data: content.into_bytes(),
                stat: rune_db::stat_facts_from(sighted.stat()),
                confirmed: sighted.is_confirmed(),
            },
        )
        .expect("enqueue materialize record");
    expect_ok(&rx, id);

    std::fs::write(&doc_id_marker, doc_id.to_string()).expect("write doc id marker");
    store.shutdown();
    std::process::exit(0);
}

/// Role (f): the sibling of [`gc_editor`] — repeatedly opens and closes
/// its OWN `Store` against the same shared path. Every `Store::open`
/// runs a best-effort startup blob sweep (`store.rs`'s doc: "One
/// startup blob-sweep batch ... after the reaper"), so this role's
/// open/close loop is what actually generates the real cross-process
/// `sweep_unreferenced_blobs` contention against `gc_editor`'s
/// concurrent orphaning — the exact gap [rune-db 8] names.
pub(crate) fn gc_sweeper() {
    let path = db_path();
    let count: usize = env_var("RUNE_DB_COUNT").parse().expect("count");
    let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
    let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));

    touch(&ready);
    wait_for_path(&go, MARKER_SAFETY_DEADLINE);

    for _ in 0..count {
        let store = open_store(&path, Box::new(|_evt| {}));
        store.shutdown();
    }

    std::process::exit(0);
}
