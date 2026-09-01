use super::*;

// Returns whether the op was actually enqueued: a re-baseline caller must
// drop `id`'s existing `db` binding on `false` rather than leave it
// standing with a baseline it can no longer refresh.
pub fn load_document(
    app: &mut App,
    id: DocumentId,
    path: &crate::resolved::ResolvedPath,
    intent: LoadIntent,
) -> bool {
    load_document_inner(app, id, path, intent, LoadErrorPolicy::Degrade)
}

// Unlike `load_document`, an error here never degrades the store: this
// runs from inside a `Fatal` teardown loop still mid-flight over other
// documents' own saves, and degrading the store from within that loop
// would let its in-flight sweep drop a later document's still-queued save
// before its own synthetic ack even lands. The re-baseline is best-effort
// bookkeeping and must never tear the whole world down.
pub fn load_document_best_effort(
    app: &mut App,
    id: DocumentId,
    path: &crate::resolved::ResolvedPath,
) -> bool {
    load_document_inner(
        app,
        id,
        path,
        LoadIntent::Rebaseline,
        LoadErrorPolicy::Ignore,
    )
}

#[derive(Clone, Copy)]
pub enum LoadIntent {
    Recover,
    Rebaseline,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadErrorPolicy {
    Degrade,
    Ignore,
}

fn load_document_inner(
    app: &mut App,
    id: DocumentId,
    path: &crate::resolved::ResolvedPath,
    intent: LoadIntent,
    on_err: LoadErrorPolicy,
) -> bool {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return false;
    }
    let Some(doc) = app.doc(id) else { return false };
    let issued_version = doc.buffer.version();
    let bound_row = doc.doc_db().map(|d| d.db_id);
    let purpose = match intent {
        LoadIntent::Recover => LoadPurpose::Recover,
        LoadIntent::Rebaseline => LoadPurpose::Rebaseline {
            expect_row: bound_row,
        },
    };
    let Some(db) = app.db.as_ref() else {
        return false;
    };
    let enqueued = db.store.load(path.as_path());
    match enqueued {
        Ok(op_id) => {
            app.db_ops
                .insert(op_id, PendingOp::load(id, issued_version, purpose));
            // A document with no binding yet starts buffering: every edit
            // committed before this op's ack lands must reach the store
            // eventually, not be dropped.
            if let Some(doc) = app.doc_mut(id)
                && matches!(doc.replica, Replica::Detached)
            {
                doc.replica = Replica::Binding {
                    base: doc.buffer.content().to_string(),
                    pending: Vec::new(),
                };
            }
            true
        }
        Err(e) => {
            if on_err == LoadErrorPolicy::Degrade {
                crate::materialize_ack::on_store_failure(app, &e.to_string());
            }
            false
        }
    }
}

// Deferred instead of enqueued while any document bound to the same
// `db_id` has a save in flight: a save from one tab invalidates the disk
// for every tab open on that file, so a probe issued now would only read
// the pre-save world and get dropped by the epoch check its ack lands into
// anyway. `FileBinding::pending_probe` records the request instead; the
// materialize-ack handler re-calls this function for every bound document
// once any of their saves resolves, so each ends up reading the disk fact
// fresh from the post-save world, exactly once.
#[must_use]
pub fn probe(app: &mut App, id: DocumentId) -> bool {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return true;
    }
    let Some(doc) = app.doc(id) else { return true };
    let Some(db_id) = doc.doc_db().map(|d| d.db_id) else {
        return true;
    };
    if doc.path().is_none() {
        return true;
    }
    if app.any_save_in_flight_for(db_id) {
        let Some(binding) = app.file_binding_mut(db_id) else {
            return false;
        };
        binding.pending_probe = true;
        return true;
    }
    if app
        .db_ops
        .values()
        .any(|pending| pending.doc == id && pending.is_probe)
    {
        return true;
    }
    let baseline_epoch = app.file_binding(db_id).map_or(0, |b| b.baseline_epoch);
    let Some(db) = app.db.as_ref() else {
        return true;
    };
    match db.store.probe(rune_db::DocId(db_id)) {
        Ok(op_id) => {
            app.db_ops
                .insert(op_id, PendingOp::probe(id, baseline_epoch));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
    }
    true
}

pub fn create_scratch(app: &mut App, id: DocumentId) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(db) = app.db.as_ref() else { return };
    match db.store.create_scratch() {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(id));
            if let Some(doc) = app.doc_mut(id)
                && matches!(doc.replica, Replica::Detached)
            {
                doc.replica = Replica::Binding {
                    base: doc.buffer.content().to_string(),
                    pending: Vec::new(),
                };
            }
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, &e.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;

    use crate::db::{Db, DbBridge, DocDb, PublishMode};
    use crate::document::Replica;

    use super::{App, probe};

    fn in_memory_db() -> Db {
        let vfs: Arc<dyn rune_vfs::Vfs + Send + Sync> = Arc::new(Mem::new());
        let clock: rune_db::ClockFn = Arc::new(std::time::SystemTime::now);
        let store =
            rune_db::Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
        let bridge = DbBridge::bootstrap();
        Db::new(store, bridge, false)
    }

    #[test]
    fn a_deferral_with_no_file_binding_to_stash_it_in_is_reported_to_the_caller() {
        let vfs: Arc<dyn rune_vfs::Vfs + Send + Sync> = Arc::new(Mem::new());
        let mut app = App::new(
            Buffer::new("hello"),
            Some(
                crate::resolved::ResolvedPath::resolve(
                    vfs.as_ref(),
                    std::path::Path::new(&PathBuf::from("/doc.md")),
                )
                .expect("the launch path resolves"),
            ),
            vfs,
            Some(in_memory_db()),
        );
        let id = app.active;
        app.doc_mut(id).expect("doc open").replica = Replica::Bound(DocDb::new(
            1,
            PublishMode::OverwriteExisting,
            rune_db::Seq(0),
        ));
        let (version, content) = {
            let doc = app.doc(id).expect("doc open");
            (doc.buffer.version(), Arc::from(doc.buffer.content()))
        };
        app.doc_mut(id)
            .expect("doc open")
            .begin_save(version, content);

        assert!(
            !probe(&mut app, id),
            "a deferral that could not be recorded must be reported, not silently dropped"
        );
    }

    #[test]
    fn a_deferral_that_is_recorded_reports_success() {
        let vfs: Arc<dyn rune_vfs::Vfs + Send + Sync> = Arc::new(Mem::new());
        let mut app = App::new(
            Buffer::new("hello"),
            Some(
                crate::resolved::ResolvedPath::resolve(
                    vfs.as_ref(),
                    std::path::Path::new(&PathBuf::from("/doc.md")),
                )
                .expect("the launch path resolves"),
            ),
            vfs,
            Some(in_memory_db()),
        );
        let id = app.active;
        app.doc_mut(id).expect("doc open").replica = Replica::Bound(DocDb::new(
            1,
            PublishMode::OverwriteExisting,
            rune_db::Seq(0),
        ));
        app.install_or_join_file_binding(1, None);
        let (version, content) = {
            let doc = app.doc(id).expect("doc open");
            (doc.buffer.version(), Arc::from(doc.buffer.content()))
        };
        app.doc_mut(id)
            .expect("doc open")
            .begin_save(version, content);

        assert!(probe(&mut app, id));
        assert!(
            app.file_binding(1)
                .expect("binding installed")
                .pending_probe,
            "the probe must actually have been deferred"
        );
    }
}
