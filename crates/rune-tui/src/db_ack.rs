//! The ack-reaction side of the [`crate::db`] bridge (split out of `db.rs`
//! to keep it under the 500-line budget): reacting to a `Load` op's ack and
//! to an `AppendEdit` ack's durable seq. [`crate::db_enqueue`] owns building
//! and submitting the ops these react to.

use crate::app::App;
use crate::db::DocDb;
use crate::document::DocumentId;
use crate::messages;
use rune_db::LoadResult;

/// The reaction to a `Load` op's ack (plan WP6.S2/S3) — routed from
/// `app::handle_db_event` once `app.db_ops` has resolved the ack's op id to
/// `id`. `issued_version` is `id`'s buffer version recorded by
/// `db_enqueue::load_document` at ENQUEUE time, on the same `PendingOp` that
/// resolved `id`, `None` only if this ack's routing entry was somehow
/// already consumed.
///
/// A `None` `saved_obs` (should not occur — see `LoadResult::saved_obs`'s
/// own doc comment) installs nothing and surfaces a status message instead
/// of binding a document to a recovery row with no CAS baseline — and drops
/// any `db` binding `id` already had, rather than leave a stale one
/// standing: this ack is explicitly declining to install a fresh one, so a
/// document may never be left bound to a baseline this reply refused to
/// supply.
///
/// `binding_only` (`PendingOp::binding_only`'s own doc comment) routes
/// straight to [`crate::db::App::rebaseline_file_binding`] and [`install_doc_
/// db`], then returns: a re-baseline `Load` is never a recovery attempt, so
/// it never touches `id`'s buffer or `last_sync` — only the shared per-file
/// baseline and `doc.db` itself (which a lost-create-race hand-off may be
/// rebinding to an entirely different row) advance.
///
/// Otherwise `recovered` is adopted into the buffer, through
/// [`crate::document::Document::hydrate`], ONLY when `issued_version` still
/// equals the buffer's CURRENT version — `Load` is asynchronous, so the
/// user may have typed into the buffer during the round trip, and
/// clobbering those keystrokes to complete a recovery binding would violate
/// the Prime Directive. When the version has moved on, `DocDb` is still
/// installed (this document's own recovery journal is real and should be
/// used going forward), but the buffer bytes are left exactly as the user
/// last typed them — this session's baseline simply anchors from the disk
/// content `db_enqueue::load_document`'s caller already read, same as
/// `recovered == disk_content` would.
///
/// A `Hydration::Refused` (the recovered draft looked truncated, or failed
/// to apply) takes the same exit as the `saved_obs == None` arm above:
/// nothing is installed, `doc.db` is dropped, the shared binding is pruned,
/// and an error is posted — a document whose recovered content this session
/// just rejected must never keep journaling against that row, or a later
/// crash recovery would replay the rejected content plus every edit made
/// since. The buffer itself is untouched either way (`hydrate` never
/// mutates it before refusing), so direct-vfs saving still works.
pub fn handle_load_ack(
    app: &mut App,
    id: DocumentId,
    load_result: LoadResult,
    issued_version: Option<u64>,
    binding_only: bool,
) {
    let Some(expect_obs) = load_result.saved_obs else {
        detach_file_binding(app, id);
        messages::error(
            app,
            "crash recovery unavailable for this tab: load returned no baseline observation",
        );
        return;
    };

    if binding_only {
        app.rebaseline_file_binding(load_result.doc_id, expect_obs);
        install_doc_db(app, id, &load_result);
        return;
    }

    let hydration = {
        let Some(doc) = app.doc_mut(id) else { return };
        if issued_version == Some(doc.buffer.version()) {
            Some(doc.hydrate(&load_result.disk_content, &load_result.recovered))
        } else {
            None
        }
    };
    let adopted = matches!(&hydration, Some(crate::document::Hydration::Adopted));
    match hydration {
        Some(crate::document::Hydration::Refused(reason)) => {
            messages::error(app, format!("crash recovery: {reason}"));
            detach_file_binding(app, id);
            return;
        }
        // A recovered draft was just installed AND the load's own fresh
        // disk sighting genuinely diverges from the baseline it was bridged
        // from — rendering this silently, with only a footer hint, once let
        // a real conflict pass unnoticed.
        // A plain unsaved edit against an unmoved disk (`BufferAhead`)
        // stays silent — every save already surfaces that ordinarily. A
        // resumable merge suppresses the `^M` invitation: the resolver is
        // about to auto-open below, so inviting a manual merge would be
        // stale the moment it posted.
        Some(crate::document::Hydration::Adopted)
            if load_result.sync.kind == rune_db::SyncKind::Diverged
                && load_result.resumable_merge.is_none() =>
        {
            messages::warn(
                app,
                "recovered unsaved changes — the file on disk has changed since \u{21c4} [^M]erge to reconcile",
            );
        }
        Some(crate::document::Hydration::Adopted) => {
            messages::info(app, "recovered unsaved changes");
        }
        Some(crate::document::Hydration::NoChange) | None => {}
    }
    // Dirty is a content comparison now (plan WP1) — `hydrate` no longer
    // marks it itself, so every hydration site re-derives it explicitly,
    // even on the `NoChange`/version-moved-on
    // branches where `hydrate` was never actually called: this document's
    // `db` binding is about to change below, which is itself a fact worth
    // re-settling the cache against.
    crate::materialize_ack::recompute_dirty(app, id);

    // Joins the shared baseline for this `db_id` — seeded from THIS load
    // only if no other document has bound it yet (`App::
    // install_or_join_file_binding`'s own doc comment): a second tab
    // opening the same file must never reset a baseline a sibling tab's own
    // save has already advanced. Done BEFORE installing `doc.db` below: the
    // join itself never touches `id`'s own document, so there is no reason
    // to interleave it with the borrow that does.
    app.install_or_join_file_binding(load_result.doc_id, expect_obs);
    install_doc_db(app, id, &load_result);
    let Some(doc) = app.doc_mut(id) else { return };
    // Plan WP2.S3: render/hint state only (see `Document::last_sync`'s own
    // doc comment) — set even on the version-moved-on branch above where
    // `hydrate` was never called: the fact this `Load` reported is still
    // true regardless of whether the buffer adopted it.
    doc.last_sync = Some(load_result.sync.kind);
    // A dead session's still-active merge is re-entered ONLY by an ack that
    // genuinely hydrated the reconstruction the merge row was matched
    // against — a skipped or refused hydration leaves the row active for
    // the next full load to re-offer.
    if adopted && let Some(resume) = load_result.resumable_merge {
        crate::merge::resume_from_store(app, id, &resume.blocks_json, resume.theirs_obs);
    }
}

/// Installs `id`'s `DocDb` for `load_result.doc_id` — shared by both
/// `handle_load_ack` branches: the ordinary recovery path (where `db_id`
/// is `id`'s own file, freshly loaded) and the `binding_only` path (where
/// it may instead be a hand-off target this document has never bound
/// before, e.g. the lost-create-race route's racer row). Either way `id`
/// is now bound to a row read straight off disk, so the next save is an
/// overwrite, never a create.
fn install_doc_db(app: &mut App, id: DocumentId, load_result: &LoadResult) {
    let Some(doc) = app.doc_mut(id) else { return };
    doc.db = Some(DocDb::new(
        load_result.doc_id,
        false,
        load_result.bridge_seq.unwrap_or(0),
    ));
}

/// Drops `id`'s `db` binding and prunes its now-possibly-unreferenced
/// shared [`crate::db::FileBinding`] — the shared exit both of
/// `handle_load_ack`'s refusal arms take (`saved_obs == None`, and
/// `Hydration::Refused`): a document a `Load` ack has just declined to
/// supply a trustworthy baseline for may never keep journaling against a
/// row this session no longer stands behind.
fn detach_file_binding(app: &mut App, id: DocumentId) {
    let old_db_id = app.doc_db_id(id);
    if let Some(doc) = app.doc_mut(id) {
        doc.db = None;
    }
    if let Some(db_id) = old_db_id {
        app.prune_file_binding(db_id);
    }
}

/// Records that `seq` was durably committed for `id`'s oldest still-pending
/// `AppendEdit` — called from `app::handle_db_event`'s `Msg::Db` handler on
/// `DbEvent::Ok { result: OpOutcome::Seq(seq), .. }`, after `app.db_ops` has
/// already resolved the ack's op id to `id`. `id` no longer live (an ack
/// racing a future close) is a correct, silent drop — the document it would
/// have updated is already gone.
pub fn resolve_append_ack(app: &mut App, id: DocumentId, seq: i64) {
    let Some(doc) = app.doc_mut(id) else { return };
    if let Some(doc_db) = doc.db.as_mut() {
        doc_db.resolve_append_ack(seq);
    }
}

/// The reaction to a `CreateScratch` op's ack (plan WP0/WP3, mid-session
/// half): `row_id` is a freshly minted, never-bound scratch row — `id`'s
/// document binds to it exactly like a recovered launch-time scratch draft
/// does (`rune-cli::open::adopt_scratch_doc`), `bind_new` true because a
/// scratch row has never been bound to a real file, so its NEXT save must
/// still go through the create-only path. `expect_obs` is `0`, the same
/// fabricated, never-queried `ObsId` `adopt_scratch_doc` uses — `bind_new`
/// skips the CAS-baseline lookup entirely. `id` no longer live (the draft
/// was closed while this op was still in flight) is a correct, silent
/// drop — `close_now` already sweeps `db_ops` of any entry pointing at a
/// closed document, but a race between that sweep and this ack landing is
/// still just a document that's gone, nothing to bind.
pub fn handle_create_scratch_ack(app: &mut App, id: DocumentId, row_id: i64) {
    let Some(doc) = app.doc_mut(id) else { return };
    doc.db = Some(DocDb::new(row_id, true, 0));
    app.install_or_join_file_binding(row_id, 0);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::db::{Db, DbBridge};
    use crate::db_enqueue::append_edit;
    use rune_core::buffer::Buffer;
    use rune_db::{ClockFn, DbEvent, OpOutcome, Store};
    use rune_vfs::{Mem, Vfs};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn in_memory_db() -> Db {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let store = Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
        let bridge = DbBridge::bootstrap();
        Db::new(store, bridge, false)
    }

    /// Plan WP1.S8: two documents each enqueue an `AppendEdit`; delivering
    /// their `DbEvent::Ok` acks (identified only by op id, via `app.db_ops`)
    /// must route each `Seq` result to the CORRECT document's `DocDb`, never
    /// crossing them.
    #[test]
    fn db_event_acks_route_to_the_correct_document_via_db_ops() {
        let mut app = App::new(
            Buffer::new("a"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id_a = app.active;
        let id_b = app.open_document(Buffer::new("b"));

        app.doc_mut(id_a).expect("doc a exists").db = Some(DocDb::new(1, true, 0));
        app.doc_mut(id_b).expect("doc b exists").db = Some(DocDb::new(2, true, 0));

        append_edit(&mut app, id_a, &[], &[], &[]);
        append_edit(&mut app, id_b, &[], &[], &[]);

        assert_eq!(app.db_ops.len(), 2);
        let op_for_a = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id_a)
            .expect("op recorded for doc a")
            .0;
        let op_for_b = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id_b)
            .expect("op recorded for doc b")
            .0;
        assert_ne!(op_for_a, op_for_b);

        // Simulate the acks arriving in reverse enqueue order — routing
        // must key off the op id, not arrival order.
        let doc_for_b = app.db_ops.remove(&op_for_b).expect("routes to doc b").doc;
        resolve_append_ack(&mut app, doc_for_b, 42);
        let doc_for_a = app.db_ops.remove(&op_for_a).expect("routes to doc a").doc;
        resolve_append_ack(&mut app, doc_for_a, 7);

        assert_eq!(
            app.doc(id_a)
                .expect("doc a exists")
                .db
                .as_ref()
                .expect("doc a has a DocDb")
                .last_known_seq,
            7
        );
        assert_eq!(
            app.doc(id_b)
                .expect("doc b exists")
                .db
                .as_ref()
                .expect("doc b has a DocDb")
                .last_known_seq,
            42
        );
        assert!(app.db_ops.is_empty());
    }

    #[test]
    fn handle_db_event_ok_seq_pops_db_ops_and_routes_to_the_right_document() {
        let mut app = App::new(
            Buffer::new("a"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id_a = app.active;
        let id_b = app.open_document(Buffer::new("b"));
        app.doc_mut(id_a).expect("doc a exists").db = Some(DocDb::new(1, true, 0));
        app.doc_mut(id_b).expect("doc b exists").db = Some(DocDb::new(2, true, 0));

        append_edit(&mut app, id_a, &[], &[], &[]);
        let op_for_a = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id_a)
            .expect("op recorded for doc a")
            .0;

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(DbEvent::Ok {
                id: op_for_a,
                result: OpOutcome::Seq(99),
            }),
            &mut effects,
        );

        assert!(
            !app.db_ops.contains_key(&op_for_a),
            "a resolved ack must be popped from db_ops"
        );
        assert_eq!(
            app.doc(id_a)
                .expect("doc a exists")
                .db
                .as_ref()
                .expect("doc a has a DocDb")
                .last_known_seq,
            99
        );
    }

    /// Review fix: a `DbEvent::Fatal` tears the whole writer thread down —
    /// every `db_ops` entry still in flight will never receive its ack, so
    /// `handle_db_event`'s `Fatal` arm must clear the map outright rather
    /// than leaving those entries as dead weight for the rest of the
    /// session.
    #[test]
    fn handle_db_event_fatal_clears_every_in_flight_db_op() {
        let mut app = App::new(
            Buffer::new("a"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id_a = app.active;
        let id_b = app.open_document(Buffer::new("b"));
        app.doc_mut(id_a).expect("doc a exists").db = Some(DocDb::new(1, true, 0));
        app.doc_mut(id_b).expect("doc b exists").db = Some(DocDb::new(2, true, 0));

        append_edit(&mut app, id_a, &[], &[], &[]);
        append_edit(&mut app, id_b, &[], &[], &[]);
        assert_eq!(app.db_ops.len(), 2, "test setup: two ops in flight");

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(DbEvent::Fatal {
                error: "writer thread died".to_string(),
            }),
            &mut effects,
        );

        assert!(
            app.db_ops.is_empty(),
            "a Fatal event must clear every in-flight db_ops entry"
        );
        assert!(
            app.db.as_ref().expect("store still present").degraded,
            "a Fatal event must still degrade the store via on_store_failure"
        );
    }

    #[test]
    fn handle_load_ack_messages_a_non_diverged_adoption() {
        let mut app = App::new(
            Buffer::new("on disk"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id = app.active;
        let issued_version = app.doc(id).expect("doc exists").buffer.version();

        let load_result = rune_db::LoadResult {
            doc_id: 1,
            renamed_from: None,
            disk_content: "on disk".to_string(),
            recovered: "recovered draft".to_string(),
            has_history: true,
            sync: rune_db::SyncState {
                kind: rune_db::SyncKind::BufferAhead,
                ancestor: None,
                ours: rune_db::Version {
                    hash: String::new(),
                    obs: None,
                },
                theirs: None,
            },
            nlink: 1,
            saved_obs: Some(1),
            bridge_seq: None,
            resumable_merge: None,
        };

        handle_load_ack(&mut app, id, load_result, Some(issued_version), false);

        assert_eq!(
            messages::newest_text(&app),
            Some("recovered unsaved changes")
        );
    }

    /// Issue #82: a `binding_only` ack for a document with live buffer
    /// edits must never adopt the recovered content or touch `last_sync` —
    /// only the shared per-file baseline and `doc.db` itself (which the
    /// lost-create-race hand-off relies on rebinding to an entirely
    /// different row, `bind_new` included) advance. Contrasts `handle_
    /// load_ack_messages_a_non_diverged_adoption` above, which drives the
    /// ordinary (`binding_only: false`) path and DOES adopt.
    #[test]
    fn binding_only_load_does_not_rehydrate() {
        let mut app = App::new(
            Buffer::new("live edits"),
            None,
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id = app.active;
        // Starts `bind_new: true`, on a DIFFERENT `db_id` than the ack
        // below carries — the exact shape the lost-create-race hand-off
        // leaves behind right before its own `binding_only` `Load` lands.
        app.doc_mut(id).expect("doc exists").db = Some(DocDb::new(3, true, 0));
        app.install_or_join_file_binding(3, 0);
        let issued_version = app.doc(id).expect("doc exists").buffer.version();

        let load_result = rune_db::LoadResult {
            doc_id: 7,
            renamed_from: None,
            disk_content: "on disk".to_string(),
            recovered: "a stale recovery row".to_string(),
            has_history: true,
            sync: rune_db::SyncState {
                kind: rune_db::SyncKind::Clean,
                ancestor: None,
                ours: rune_db::Version {
                    hash: String::new(),
                    obs: None,
                },
                theirs: None,
            },
            nlink: 1,
            saved_obs: Some(42),
            bridge_seq: Some(9),
            resumable_merge: None,
        };

        handle_load_ack(&mut app, id, load_result, Some(issued_version), true);

        assert_eq!(
            app.doc(id).expect("doc exists").buffer.content(),
            "live edits",
            "binding_only must never adopt recovered content into the buffer"
        );
        let doc_db = app
            .doc(id)
            .expect("doc exists")
            .db
            .as_ref()
            .expect("doc.db must be rebound to the hand-off's target row");
        assert_eq!(
            doc_db.db_id, 7,
            "a binding_only ack rebinds doc.db to the ack's OWN db_id"
        );
        assert!(
            !doc_db.bind_new,
            "a binding_only ack always installs bind_new: false"
        );
        assert_eq!(doc_db.last_known_seq, 9);
        assert_eq!(
            app.doc(id).expect("doc exists").last_sync,
            None,
            "binding_only must never touch last_sync"
        );
        assert_eq!(
            app.file_binding(7).expect("binding exists").expect_obs,
            42,
            "the shared per-file baseline must advance for the ack's OWN db_id"
        );
    }

    /// Issue #82: the re-baseline `Load` a `saved: None` `MaterializeRecord`
    /// ack enqueues (`materialize_ack::reactions`) must actually advance
    /// `expect_obs` once its own ack lands, and clear `pending_
    /// rebaseline_hash` — not fall into `install_or_join_file_binding`'s
    /// join semantics, which would silently drop the fresh observation for
    /// a `db_id` this process already has a binding for.
    #[test]
    fn rebaseline_load_advances_expect_obs() {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        vfs.save_atomic(Path::new("/doc.md"), b"hello")
            .expect("seed doc.md");
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let bridge = DbBridge::bootstrap();
        let store =
            Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("open store");

        store.load(Path::new("/doc.md")).expect("enqueue load");
        let load = match bridge.wait_for_bootstrap_event(|_| true) {
            DbEvent::Ok {
                result: OpOutcome::Load(load),
                ..
            } => *load,
            other => panic!("expected a Load ack, got {other:?}"),
        };

        let mut app = App::new(
            Buffer::new("hello"),
            Some(PathBuf::from("/doc.md")),
            Arc::clone(&vfs),
            Some(Db::new(store, Arc::clone(&bridge), false)),
        );
        let id = app.active;
        app.doc_mut(id).expect("doc exists").db = Some(DocDb::new(load.doc_id, false, 0));
        app.install_or_join_file_binding(load.doc_id, load.saved_obs.unwrap_or(0));

        // Simulates exactly the state a `saved: None` `MaterializeRecord`
        // ack leaves behind (`materialize_ack.rs`'s own `record_outcome`
        // `Err` arm) — reproducing the transient writer-queue failure that
        // produces it for real would make this test racy against the
        // writer thread (same rationale `force_save.rs`'s own lost-
        // bookkeeping fixture states).
        app.file_binding_mut(load.doc_id)
            .expect("binding exists")
            .pending_rebaseline_hash = Some(rune_db::hash_bytes(b"hello"));

        // An external rewrite between the lost bookkeeping and the
        // re-baseline `Load` below, so the fresh observation this test
        // asserts against is genuinely NEW, not incidentally identical to
        // the seed `Load`'s own.
        vfs.save_atomic(Path::new("/doc.md"), b"hello again")
            .expect("external rewrite");

        let enqueued =
            crate::db_enqueue::load_document_best_effort(&mut app, id, Path::new("/doc.md"));
        assert!(
            enqueued,
            "the re-baseline Load must enqueue against a live, non-degraded store"
        );

        let rebaseline_evt = bridge.wait_for_bootstrap_event(|evt| {
            matches!(
                evt,
                DbEvent::Ok {
                    result: OpOutcome::Load(_),
                    ..
                }
            )
        });
        let fresh_obs = match &rebaseline_evt {
            DbEvent::Ok {
                result: OpOutcome::Load(load),
                ..
            } => load.saved_obs.expect("a fresh load carries a baseline"),
            other => panic!("expected a Load ack, got {other:?}"),
        };

        let mut effects = crate::runtime::Effects::default();
        crate::app::update(
            &mut app,
            crate::runtime::Msg::Db(rebaseline_evt),
            &mut effects,
        );

        let binding = app
            .file_binding(load.doc_id)
            .expect("the shared binding must survive a binding_only re-baseline");
        assert_eq!(
            binding.expect_obs, fresh_obs,
            "expect_obs must advance to the re-baseline Load's own fresh observation"
        );
        assert!(
            binding.pending_rebaseline_hash.is_none(),
            "a landed re-baseline must clear the stashed echo hash"
        );
    }
}
