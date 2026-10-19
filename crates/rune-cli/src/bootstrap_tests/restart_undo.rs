use super::dead_session_recovery::{db_path_under, mark_every_session_dead};
use super::*;
use rune_core::buffer::AppliedEdit;
use rune_core::undo::EditKind;
use rune_db::{BindingToken, EditBatch, OpOutcome, Seq, TokenOrigin};

fn wait_ok(bridge: &rune_tui::db::DbBridge, op: u64) -> OpOutcome {
    match bridge.wait_for_bootstrap_event(|evt| match evt {
        rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == op,
        rune_db::DbEvent::Fatal { .. } => true,
    }) {
        rune_db::DbEvent::Ok { result, .. } => result,
        other => panic!("expected an Ok ack, got {other:?}"),
    }
}

/// A previous run that typed `typed` one chunk at a time over `path`'s
/// content and then died without saving.
fn dead_session_that_typed(
    db_path: &Path,
    vfs: &Arc<dyn Vfs + Send + Sync>,
    path: &Path,
    typed: &[&str],
) {
    let bridge = rune_tui::db::DbBridge::bootstrap();
    let (store, _warning) =
        rune_db::Store::open(db_path, Arc::clone(vfs), bridge.on_event()).expect("open store");
    let doc_id = match wait_ok(&bridge, store.load(path).expect("enqueue load")) {
        OpOutcome::Load(load) => load.doc_id,
        other => panic!("expected a Load ack, got {other:?}"),
    };
    let token = BindingToken::next();
    let mut len = vfs.read(path).expect("read the file").len();
    for chunk in typed {
        let edit = AppliedEdit {
            start: len,
            end: len,
            deleted: String::new(),
            insert: (*chunk).to_string(),
        };
        len += chunk.len();
        let op = store
            .append_edit(
                doc_id,
                token,
                &TokenOrigin::at(Seq(0)),
                EditBatch {
                    edits: &[edit],
                    cursors_before: &[],
                    cursors_after: &[],
                    kind: EditKind::Other,
                },
            )
            .expect("enqueue append");
        assert!(matches!(wait_ok(&bridge, op), OpOutcome::Seq(_)));
    }
    store.shutdown();
}

#[test]
fn a_file_relaunched_after_a_crash_undoes_the_dead_sessions_edits_step_by_step() {
    let home = ScratchHome::new("restart-undo");
    let db_path = db_path_under(&home.0);
    let path = Path::new("/vault/notes.md");
    let mem = Mem::new();
    mem.save_atomic(path, b"hello").expect("seed the file");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);
    dead_session_that_typed(&db_path, &vfs, path, &[" world", "!"]);
    mark_every_session_dead(&db_path);

    let mut app = bootstrap(
        &vfs,
        vec![OsString::from(path)].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("relaunch should succeed");

    let id = app.active;
    assert_eq!(app.active_doc().buffer.content(), "hello world!");
    rune_tui::commands::edit::undo(&mut app, id);
    assert_eq!(app.active_doc().buffer.content(), "hello world");
    rune_tui::commands::edit::undo(&mut app, id);
    assert_eq!(app.active_doc().buffer.content(), "hello");
}
