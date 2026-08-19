use crate::app::App;
use crate::document::DocumentId;
use crate::messages;

pub(super) fn handle_committed_ack(
    app: &mut App,
    id: DocumentId,
    pending_version: Option<u64>,
    saved: Option<&rune_db::Observation>,
    raced: bool,
) {
    // A committed bind-new create is where an untitled draft finally
    // gets its path — only now, after the no-clobber publish actually
    // succeeded (see `bind_new_now`'s docs).
    if let Some(path) = app
        .doc_mut(id)
        .and_then(crate::document::Document::take_bind_target)
    {
        if let Some(doc) = app.doc_mut(id) {
            doc.bind_path(path);
        }
        if app.active == id {
            let name = app.doc(id).map(crate::title::name_for).unwrap_or_default();
            app.title.seed(&name);
        }
    }
    // Once the bytes are published, the target exists, so the next save
    // is an overwrite — regardless of whether the bookkeeping that would
    // have supplied `saved` (and so a fresh CAS baseline) survived.
    // `publish_mode` stays on THIS document's own `DocDb` (never shared — a
    // scratch row racing to bind is claimed by exactly one document),
    // but the epoch/baseline below live on the SHARED `FileBinding` for
    // `db_id`, so every OTHER tab open on the same file sees the same
    // advance a single tab's own save just produced — closing the
    // false-conflict class where a second tab's next save compares
    // against a stale per-tab copy that never learned about this commit,
    // instead of the file's true current baseline.
    let db_id = app.doc_db_id(id);
    if let Some(doc_db) = app.doc_mut(id).and_then(|d| d.doc_db_mut()) {
        doc_db.publish_mode = crate::db::PublishMode::OverwriteExisting;
    }
    if let Some(binding) = app.doc_file_binding_mut(id) {
        // The publish just committed — any `Probe` issued before this
        // reply lands, by ANY document bound to `db_id`, is now
        // describing a disk that no longer exists; bumping the epoch
        // here is what makes `db_dispatch`'s `OpOutcome::Sync` arm drop
        // such a stale reply instead of trusting it.
        binding.baseline_epoch = binding.baseline_epoch.wrapping_add(1);
    }
    if let Some(saved) = saved
        && let Some(binding) = app.doc_file_binding_mut(id)
    {
        binding.expect_obs = Some(saved.id);
        binding.pending_rebaseline_hash = None;
    }
    let was_hardlinked = app.doc(id).is_some_and(|d| d.nlink.is_some_and(|n| n > 1));
    if was_hardlinked {
        messages::info(
            app,
            "saved \u{2014} this file was hard-linked; its other names still hold the previous content",
        );
    }
    if let Some(doc) = app.doc_mut(id) {
        doc.nlink = saved.and_then(|o| o.nlink);
    }
    // Resolved BEFORE the `saved: None` re-baseline below: that arm may
    // enqueue a `Load`, and a `Load` enqueue failure sweeps every
    // document with `save_in_flight` (`db_enqueue::load_document` ->
    // `on_store_failure`) and abandons its capture. Resolving this
    // save first means that sweep can no longer be re-entrant against
    // the very save it is trying to finish — a physically successful
    // publish must promote into `saved_content` regardless of whether
    // the re-baseline that follows succeeds.
    if let Some(doc) = app.doc_mut(id) {
        match pending_version {
            Some(version) => {
                doc.finish_save_ok(version);
            }
            None => doc.abandon_save(),
        }
    }
    if saved.is_none() {
        // A document may never be left with a store binding that
        // cannot serve its next save. The bookkeeping that would
        // have supplied a fresh CAS baseline was lost (`record_
        // outcome`'s synthetic-commit arms), so the overwrite mode
        // above paired with the STALE `expect_obs` a scratch row
        // installs would make the very next save's `materialize_
        // prepare` look up an observation that was never recorded.
        // Re-baseline via an ordinary `Load` of the path that was
        // just published when the store can still serve one — its
        // ack installs a real `expect_obs`. When it cannot (no
        // store, or degraded), this document's binding can no
        // longer serve a save at all: drop it so every later save
        // takes the direct-vfs path instead, which always works.
        // Journaling is already dead in that state (`append_edit`
        // early-returns on degraded), so the binding costs nothing
        // kept dropped and only blocks saving kept standing. The
        // `Load` is a re-baseline (never a recovery adoption) — this
        // is anchoring a CAS baseline for content already known, not
        // recovering anything. Uses the best-effort enqueue, never the
        // degrading one: this call may run once per document inside a
        // `DbEvent::Fatal` teardown loop still mid-flight over OTHER
        // documents' own saves, and degrading the store from inside
        // that loop would drop a later document's still-queued
        // `save_pending` before its own synthetic ack even lands.
        let path = app.doc(id).and_then(|d| d.file_path.clone());
        let store_usable = app.db.as_ref().is_some_and(|db| !db.degraded);
        let re_baselined = match (path, store_usable) {
            (Some(path), true) => crate::db_enqueue::load_document_best_effort(app, id, &path),
            _ => false,
        };
        // Re-checked AFTER the attempt, not just relied on the
        // pre-check above: an enqueue failure already degrades the
        // store synchronously, but this also catches the store having
        // gone down for some other reason in between.
        let still_usable = app.db.as_ref().is_some_and(|db| !db.degraded);
        if !re_baselined || !still_usable {
            if let Some(doc) = app.doc_mut(id) {
                doc.replica = crate::document::Replica::Detached;
            }
            // `db_id` is this document's own binding as captured above
            // `load_document_best_effort` may have already installed a
            // FRESH `DocDb` on a successful re-baseline (still cleared
            // right back out by the drop just above when the store
            // itself turned out unusable) — either way, the `db_id` this
            // save actually published under is what may have just lost
            // its last referencing document.
            if let Some(db_id) = db_id {
                app.prune_file_binding(db_id);
            }
        }
    }
    if raced {
        messages::info(
            app,
            "saved \u{2014} a concurrent external change was overwritten; its bytes were preserved",
        );
    }
}
