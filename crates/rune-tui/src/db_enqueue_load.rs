use super::*;

/// Enqueues a `Load` op hydrating `id` (already bound to `path`, an
/// existing file just read straight off disk — `workspace::open_path`'s one
/// call site) through the app-wide recovery store, closing the "Explorer-
/// opened documents get no recovery journal" gap. Records `id`'s
/// buffer version at the moment the load is ISSUED, alongside the routing
/// entry, in one `PendingOp` in `app.db_ops` — `app::handle_db_event`'s
/// `Load` arm needs both to decide, on the ack, whether adopting the
/// recovered content is still safe (see [`crate::db_ack::handle_load_ack`]'s
/// docs). `intent` becomes that `PendingOp`'s [`crate::db::LoadPurpose`],
/// carrying the row `id` is bound to right now — see that type's own doc
/// comment for why a `Rebaseline` call must name it, on both this enqueue
/// and the writer thread. A degraded store enqueues nothing — there is no
/// trustworthy recovery journal to bind this document to either way.
///
/// Returns whether the op was actually enqueued: a re-baseline caller
/// (`materialize_ack::reactions`) must drop `id`'s existing `db` binding
/// on `false` rather than leave it standing with a baseline it can no
/// longer refresh; the other call sites, which have no binding yet to
/// protect, are unaffected either way.
pub fn load_document(app: &mut App, id: DocumentId, path: &Path, intent: LoadIntent) -> bool {
    load_document_inner(app, id, path, intent, LoadErrorPolicy::Degrade)
}

/// The re-baseline counterpart to [`load_document`]: same enqueue, but an
/// error NEVER calls `on_store_failure` — used only by the committed-outcome
/// re-baseline in `materialize_ack::reactions`, which may run once per
/// document inside a `DbEvent::Fatal` teardown loop still mid-flight over
/// OTHER documents' own saves. Degrading the store from inside that loop
/// would let `on_store_failure`'s in-flight sweep drop a LATER document's
/// still-queued `save_pending` before its own synthetic ack even lands —
/// the re-baseline is best-effort bookkeeping and must never tear the whole
/// world down. The caller already treats a `false` return as "drop this
/// document's binding, the next save falls back to direct-vfs", and the
/// outer `Fatal` arm reports the store failure itself once the whole loop
/// has finished.
pub fn load_document_best_effort(app: &mut App, id: DocumentId, path: &Path) -> bool {
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
    path: &Path,
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
    let enqueued = db.store.load(path);
    match enqueued {
        Ok(op_id) => {
            app.db_ops
                .insert(op_id, PendingOp::load(id, issued_version, purpose));
            // A document with no binding yet starts buffering: every edit
            // committed before this op's ack lands must reach the store
            // eventually, not be dropped. A re-baseline/hand-off
            // `Load` (`load_document_best_effort`, or a `Rebaseline` call
            // against an already-`Bound` document) targets a document that
            // is already `Bound` or `Detached`-by-design, so this is a no-op
            // for those — only a genuinely fresh, never-bound document
            // transitions here.
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

/// Enqueues a `Probe` op refreshing `id`'s disk fact — called
/// from `workspace::switch_to` for a document with both a `db` binding and a
/// `file_path` (nothing to probe for a pathless draft or one with no
/// recovery journal). Skips enqueueing if `id` already has a probe in
/// flight (`PendingOp::is_probe`, `db.rs`'s own doc comment) — a rapid
/// sequence of tab switches back onto the same document must not stack
/// redundant probes. The resulting `SyncState` lands as `OpOutcome::Sync`,
/// handled in `db_dispatch::handle_db_event`.
///
/// Deferred instead of enqueued while ANY document bound to the same
/// `db_id` has a save in flight — a save from one tab invalidates the disk
/// for every tab open on that file, not only the one saving, so a probe
/// issued now would only read the pre-save world and get
/// dropped by the epoch check its ack lands into anyway (`db_dispatch`'s
/// `OpOutcome::Sync` arm). The shared `FileBinding::pending_probe` records
/// the request instead; `materialize_ack::handle_materialize_ack`'s tail
/// re-calls this function, for every document still bound to `db_id`, once
/// ANY of their saves resolves — so the disk fact each of them ends up with
/// is read fresh from the POST-save world, exactly once.
#[must_use]
pub fn probe(app: &mut App, id: DocumentId) -> bool {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return true;
    }
    let Some(doc) = app.doc(id) else { return true };
    let Some(db_id) = doc.doc_db().map(|d| d.db_id) else {
        return true;
    };
    if doc.file_path.is_none() {
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

/// Enqueues a `CreateScratch` op registering `id` — a freshly minted
/// untitled draft (`workspace::new_untitled_document`'s one call site) — as
/// its own scratch row in the recovery store, closing the "an untitled
/// draft minted mid-session has no journal" gap. Mirrors
/// `load_document`'s enqueue-then-record shape: a degraded or absent store
/// enqueues nothing, leaving the document `Detached` and the draft exactly
/// as unpreserved as it is today — `App::is_preserved` already reports that
/// honestly, so there is nothing else to gate here. The ack binds the
/// document's `DocDb` (`db_ack::handle_create_scratch_ack`); until it lands
/// the draft is simply not yet preserved, same as any other in-flight
/// recovery op.
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
            Some(PathBuf::from("/doc.md")),
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
            Some(PathBuf::from("/doc.md")),
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
