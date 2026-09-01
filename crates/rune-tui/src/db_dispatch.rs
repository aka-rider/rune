use crate::app::App;
use crate::db::PendingOp;
use crate::materialize_ack;
use crate::runtime::Effects;
use rune_db::DbEvent;

fn with_pending_op(app: &mut App, op_id: u64, react: impl FnOnce(&mut App, PendingOp)) {
    if let Some(pending) = app.db_ops.remove(&op_id) {
        react(app, pending);
    }
}

pub(crate) fn handle_db_event(app: &mut App, evt: DbEvent, effects: &mut Effects) {
    match evt {
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Seq(seq),
        } => with_pending_op(app, op_id, |app, pending| {
            crate::db_ack::resolve_append_ack(app, pending.doc, seq);
        }),
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::MaterializePrep(prep),
        } => with_pending_op(app, op_id, |app, pending| {
            materialize_ack::handle_prepare_ack(
                app,
                pending.doc,
                op_id,
                pending.baseline_epoch,
                *prep,
                effects,
            );
        }),
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Materialize(mat),
        } => with_pending_op(app, op_id, |app, pending| {
            materialize_ack::handle_materialize_ack_for_op(app, pending.doc, op_id, *mat, effects);
        }),
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Rename(outcome),
        } => {
            app.db_ops.remove(&op_id);
            crate::rename::handle_rename_ack(app, op_id, *outcome, effects);
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Load(load_result),
        } => with_pending_op(app, op_id, |app, pending| {
            crate::db_ack::handle_load_ack(
                app,
                pending.doc,
                *load_result,
                pending.issued_version,
                pending.load_purpose,
                effects,
            );
        }),
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::Sync(state),
        } => {
            let Some(pending) = app.db_ops.remove(&op_id) else {
                return;
            };
            let current_epoch = app
                .doc_file_binding(pending.doc)
                .map(|binding| binding.baseline_epoch);
            if pending.baseline_epoch != current_epoch {
                let _ = crate::db_enqueue::probe(app, pending.doc);
                return;
            }
            if let Some(doc) = app.doc_mut(pending.doc) {
                doc.last_sync = Some(state.kind);
            }
            crate::guard::retract_disk_conflict_on_convergence(app, pending.doc, state.kind);
            crate::merge::retract_active_on_convergence(app, pending.doc, state.kind);
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::MergePrep(prep),
        } => {
            with_pending_op(app, op_id, |app, pending| {
                crate::merge::handle_merge_prep_ack(
                    app,
                    pending.doc,
                    pending.merge_gen,
                    *prep,
                    effects,
                );
            });
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::SnapshotRowId(_),
        } => {
            app.db_ops.remove(&op_id);
        }
        DbEvent::Ok {
            id: op_id,
            result: rune_db::OpOutcome::ScratchDocId(doc_id),
        } => {
            with_pending_op(app, op_id, |app, pending| {
                crate::db_ack::handle_create_scratch_ack(app, pending.doc, doc_id.0);
            });
        }
        DbEvent::Ok {
            id: op_id,
            result:
                rune_db::OpOutcome::None
                | rune_db::OpOutcome::Ids(_)
                | rune_db::OpOutcome::Reconstructed(_)
                | rune_db::OpOutcome::Observation(_),
        } => {
            app.db_ops.remove(&op_id);
            app.search_history.ack(op_id);
            app.command_history.ack(op_id);
        }
        DbEvent::Err { id: op_id, error } => {
            let pending = app.db_ops.remove(&op_id);
            if app.search_history.fail(op_id) {
                crate::messages::error(app, format!("search history not saved: {error}"));
                return;
            }
            if app.command_history.fail(op_id) {
                crate::messages::error(app, format!("command history not saved: {error}"));
                return;
            }
            crate::rename::fail_op(app, op_id, error.clone(), effects);
            if let Some(pending) = pending
                && pending.doc_scoped
            {
                let text = match app.doc(pending.doc) {
                    Some(doc) => format!("{}: {error}", doc.file_name()),
                    None => error,
                };
                crate::messages::error(app, text);
                return;
            }
            materialize_ack::on_store_failure(app, &error);
        }
        DbEvent::Fatal { error } => {
            crate::rename::fail_all(app, error.clone(), effects);
            materialize_ack::on_store_failure(app, &error);
            // A `Fatal` tears down the whole writer thread, so none of
            // these in-flight entries will ever receive their ack; left
            // alone they'd leak for the rest of the session. A missing
            // `db_ops` entry is already a plain no-op elsewhere, so
            // clearing the map here drops no ack that could still land.
            app.db_ops.clear();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db::{Db, DbBridge, DocDb};
    use crate::document::Replica;
    use rune_core::buffer::Buffer;
    use rune_db::{BlobHash, ClockFn, Store, SyncKind, SyncState, Version};
    use rune_vfs::{Mem, Vfs};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn in_memory_db() -> Db {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let store = Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
        let bridge = DbBridge::bootstrap();
        Db::new(store, bridge, false)
    }

    fn clean_state() -> SyncState {
        SyncState {
            kind: SyncKind::Clean,
            ancestor: None,
            ours: Version {
                hash: BlobHash("same".to_string()),
                obs: None,
            },
            theirs: None,
        }
    }

    #[test]
    fn stale_probe_ack_rearms() {
        let vfs = Arc::new(Mem::new());
        let launch = crate::resolved::ResolvedPath::resolve(
            vfs.as_ref(),
            std::path::Path::new("/vault/note.md"),
        )
        .expect("the launch path resolves");
        let mut app = App::new(Buffer::new("body"), Some(launch), vfs, Some(in_memory_db()));
        let id = app.active;
        app.doc_mut(id).expect("doc exists").replica = Replica::Bound(DocDb::new(
            1,
            crate::db::PublishMode::OverwriteExisting,
            rune_db::Seq(0),
        ));
        app.install_or_join_file_binding(1, None);

        let _ = crate::db_enqueue::probe(&mut app, id);
        let op_id = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.is_probe)
            .expect("probe enqueued")
            .0;
        assert_eq!(
            app.db_ops.get(&op_id).expect("op recorded").baseline_epoch,
            Some(0)
        );

        app.file_binding_mut(1)
            .expect("binding exists")
            .baseline_epoch = 1;

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(rune_db::DbEvent::Ok {
                id: op_id,
                result: rune_db::OpOutcome::Sync(Box::new(clean_state())),
            }),
            &mut effects,
        );

        assert!(
            app.doc(id).expect("doc exists").last_sync.is_none(),
            "a stale-epoch ack must never overwrite last_sync"
        );
        assert!(
            !app.db_ops.contains_key(&op_id),
            "the stale ack must still be popped from db_ops"
        );
        let reissued = app
            .db_ops
            .values()
            .find(|pending| pending.is_probe && pending.doc == id)
            .expect("a fresh probe must be re-issued");
        assert_eq!(
            reissued.baseline_epoch,
            Some(1),
            "the re-issued probe must record the CURRENT epoch"
        );
    }

    #[test]
    fn stale_prepare_ack_abandons_the_save_without_reclassifying() {
        let vfs = Arc::new(Mem::new());
        let launch = crate::resolved::ResolvedPath::resolve(
            vfs.as_ref(),
            std::path::Path::new("/vault/note.md"),
        )
        .expect("the launch path resolves");
        let mut app = App::new(Buffer::new("body"), Some(launch), vfs, Some(in_memory_db()));
        let id = app.active;
        app.doc_mut(id).expect("doc exists").replica = Replica::Bound(DocDb::new(
            1,
            crate::db::PublishMode::OverwriteExisting,
            rune_db::Seq(0),
        ));
        app.install_or_join_file_binding(1, None);
        let op_id = 7;
        app.doc_mut(id).expect("doc exists").begin_prepare(
            1,
            Arc::from("body"),
            crate::document::PublishParams {
                path: PathBuf::from("/vault/note.md"),
                publish_mode: crate::db::PublishMode::OverwriteExisting,
                db_id: 1,
                seq: 0,
                mode: crate::save::SaveMode::Normal,
                bind_target: None,
            },
            op_id,
        );
        app.db_ops.insert(op_id, PendingOp::prepare(id, 0));

        app.file_binding_mut(1)
            .expect("binding exists")
            .baseline_epoch = 1;

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(rune_db::DbEvent::Ok {
                id: op_id,
                result: rune_db::OpOutcome::MaterializePrep(Box::new(
                    rune_db::MaterializePrep::Overwrite {
                        bound_path: "/vault/note.md".to_string(),
                        expect_hash: BlobHash("stale".to_string()),
                        sync: SyncKind::Diverged,
                    },
                )),
            }),
            &mut effects,
        );

        assert!(
            app.doc(id).expect("doc exists").last_sync.is_none(),
            "a stale-epoch prepare verdict must never overwrite last_sync"
        );
        assert!(
            app.guard.is_none(),
            "a stale-epoch prepare refusal must never raise the disk-conflict Guard"
        );
        assert!(
            !app.doc(id).expect("doc exists").save_in_flight(),
            "the save attempt must be abandoned"
        );
        let reissued = app
            .db_ops
            .values()
            .find(|pending| pending.is_probe && pending.doc == id)
            .expect("a fresh probe must be issued for the post-rewrite world");
        assert_eq!(reissued.baseline_epoch, Some(1));
    }

    #[test]
    fn a_failed_command_history_write_clears_the_dedup_guard() {
        let mut app = App::new(Buffer::new("body"), None, Arc::new(Mem::new()), None);
        app.command_history.last_persisted = Some("save".to_string());
        app.command_history.ops.insert(42);

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(rune_db::DbEvent::Err {
                id: 42,
                error: "disk full".to_string(),
            }),
            &mut effects,
        );

        assert!(
            app.command_history.last_persisted.is_none(),
            "a failed write must clear the dedup guard so a retry is not skipped forever"
        );
        assert!(!app.command_history.ops.contains(&42));
    }

    #[test]
    fn a_failed_search_history_write_clears_the_dedup_guard() {
        let mut app = App::new(Buffer::new("body"), None, Arc::new(Mem::new()), None);
        app.search_history.last_persisted = Some("hi".to_string());
        app.search_history.ops.insert(7);

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(rune_db::DbEvent::Err {
                id: 7,
                error: "disk full".to_string(),
            }),
            &mut effects,
        );

        assert!(
            app.search_history.last_persisted.is_none(),
            "a failed write must clear the dedup guard so a retry is not skipped forever"
        );
        assert!(!app.search_history.ops.contains(&7));
    }
}
