//! The ack-reaction side of the [`crate::db`] bridge (split out of
//! `db.rs`): reacting to a `Load` op's ack and to an `AppendEdit` ack's
//! durable seq, plus the one chokepoint every `Replica::Bound` install
//! funnels through. [`crate::db_enqueue`] owns building and submitting the
//! ops these react to.

use crate::app::App;
use crate::db::{DocDb, LoadPurpose, PublishMode};
use crate::document::{DocumentId, Replica, ReplicaStep};
use crate::messages;
use rune_core::buffer::AppliedEdit;
#[cfg(test)]
use rune_db::BlobHash;
use rune_db::LoadResult;

/// The reaction to a `Load` op's ack — routed from
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
/// A `Rebaseline` purpose (`crate::db::LoadPurpose`'s own doc comment)
/// routes straight to [`crate::db::App::rebaseline_file_binding`] and
/// [`install_doc_db`], then returns: a re-baseline `Load` is never a
/// recovery attempt, so it never touches `id`'s buffer or `last_sync` —
/// only the shared per-file baseline and the document's own `DocDb` itself
/// (which a lost-create-race hand-off may be rebinding to an entirely
/// different row) advance. Whether the writer kept or restarted this row's
/// undo numbering follows from the row named when the op was enqueued, so
/// both sides decide it from one fact. A document that moved to another row
/// meanwhile keeps the binding it now has — the reply describes a row it no
/// longer holds — and only the exactness of its deep undo is at risk, which
/// is warned about rather than paid for by dropping a live binding.
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
/// nothing is installed, the replica is `Detached`, the shared binding is pruned,
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
    purpose: LoadPurpose,
) {
    let Some(expect_obs) = load_result.saved_obs else {
        detach_file_binding(app, id);
        messages::error(
            app,
            "crash recovery unavailable for this tab: load returned no baseline observation",
        );
        return;
    };

    if let LoadPurpose::Rebaseline { expect_row } = purpose {
        let loaded_row = load_result.doc_id.0;
        let writer_kept_numbering = expect_row == Some(loaded_row);
        let still_bound_there = app.doc_db_id(id) == Some(loaded_row);
        if writer_kept_numbering && !still_bound_there {
            messages::warn(
                app,
                "undo history for this tab may no longer be recoverable step by step \u{2014} save to settle it",
            );
            return;
        }
        app.rebaseline_file_binding(loaded_row, expect_obs);
        let mode = if writer_kept_numbering {
            BindMode::Preserved
        } else {
            BindMode::Restarted { adopted: false }
        };
        let pending = install_doc_db(app, id, &load_result, mode);
        crate::db_enqueue::replay_pending(app, id, pending);
        return;
    }

    if app.merge.doc() == Some(id) {
        crate::merge::auto_exit(app);
    }
    let hydration = {
        let Some(doc) = app.doc_mut(id) else { return };
        if issued_version == Some(doc.buffer.version()) {
            Some(doc.hydrate(
                &load_result.disk_content,
                &load_result.recovered.content,
                &load_result.recovered.cursors,
            ))
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
    // Joins the shared baseline for this `db_id` — seeded from THIS load
    // only if no other document has bound it yet (`App::
    // install_or_join_file_binding`'s own doc comment): a second tab
    // opening the same file must never reset a baseline a sibling tab's own
    // save has already advanced. Done BEFORE installing the DocDb below: the
    // join itself never touches `id`'s own document, so there is no reason
    // to interleave it with the borrow that does.
    app.install_or_join_file_binding(load_result.doc_id.0, Some(expect_obs));
    let pending = install_doc_db(app, id, &load_result, BindMode::Restarted { adopted });
    crate::db_enqueue::replay_pending(app, id, pending);
    let Some(doc) = app.doc_mut(id) else { return };
    // Render/hint state only (see `Document::last_sync`'s own doc comment)
    // — set even on the version-moved-on branch above where `hydrate` was
    // never called: the fact this `Load` reported is still true regardless
    // of whether the buffer adopted it.
    doc.last_sync = Some(load_result.sync.kind);
    doc.nlink = Some(load_result.nlink);
    warn_hard_links(app, load_result.nlink);
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
/// is `id`'s own file, freshly loaded) and the re-baseline path (where
/// it may instead be a hand-off target this document has never bound
/// before, e.g. the lost-create-race route's racer row). Either way `id`
/// is now bound to a row read straight off disk, so the next save is an
/// overwrite, never a create. `mode` is what the writer thread just did to
/// this row's local-position numbering. Returns the
/// [`ReplicaStep`]s the caller replays via `db_enqueue::replay_pending` —
/// done AFTER `id` is `Bound`, so a document is only ever `Binding` for
/// the length of one round trip.
fn install_doc_db(
    app: &mut App,
    id: DocumentId,
    load_result: &LoadResult,
    mode: BindMode,
) -> Vec<ReplicaStep> {
    let doc_db = DocDb::new(
        load_result.doc_id.0,
        PublishMode::OverwriteExisting,
        load_result.bridge_seq.unwrap_or(rune_db::Seq(0)),
    );
    bind_document_row(app, id, doc_db, &load_result.recovered.content, mode)
}

/// What the writer thread did to the bound row's local undo-position
/// numbering, which this side must mirror. `adopted` is whether this load's
/// own hydration adopted the recovered content — the extra local journal
/// entry the writer never sees. Under `Preserved` the existing mapping is
/// still the true one; re-deriving it would name a different entry than the
/// writer resolves.
#[derive(Clone, Copy)]
enum BindMode {
    Restarted { adopted: bool },
    Preserved,
}

/// What `id`'s replaced `DocDb` carries into a re-bind: its identity (to
/// tell a same-row re-baseline from a rebind), its lagging durable-head
/// estimate, any still-unflushed re-base bridge, and the undo mapping —
/// which is carried only into a [`BindMode::Preserved`] bind, the one bind
/// the writer thread does not restart the row's numbering at.
struct PriorBinding {
    db_id: i64,
    last_known_seq: rune_db::Seq,
    pending_rebase: Option<ReplicaStep>,
    undo_offset: i64,
    undo_floor: i64,
    appends_sent: i64,
}

/// The one chokepoint every fresh `Replica::Bound` install funnels through
/// (`install_doc_db` above, [`bind_scratch_doc`]/[`adopt_scratch_doc`] and
/// [`bind_loaded_doc`] below). `row_content` is what the bound row's
/// durable journal reconstructs to at this instant — a `Load`'s
/// `recovered`, the empty string for a freshly minted scratch row.
///
/// The writer thread restarts a row's local-position numbering at every
/// bind EXCEPT the same-row re-baseline it was asked to preserve
/// ([`BindMode`]), so a `Restarted` bind re-derives the undo mapping here
/// from the facts on the ground: the writer's entries after this
/// bind are the appends still in flight past a same-row re-baseline
/// (counted off `app.db_ops`), plus the re-base bridge and window steps
/// about to be enqueued; `undo_offset` is the local journal position none
/// of those cover, and `undo_floor` marks where exact resolution ends and
/// `db_enqueue::move_undo_pos`'s forward re-base takes over
/// (`DocDb::undo_offset`/`undo_floor`/`appends_sent`'s own doc comments).
/// For a `Restarted` same-row re-baseline, writer position 0 is
/// `bridge_seq` or `0` — a heal-adopt reload journals no bridge, so
/// position 0 does NOT reconstruct to the buffer and the floor starts at 1.
///
/// A `Preserved` bind carries the whole mapping across instead: the local
/// journal gained nothing across such a load, so re-deriving would renumber
/// positions the writer still resolves the old way and a deep undo would
/// land on a neighbouring seq rather than its own.
///
/// Binding to a DIFFERENT row — a first bind, or a hand-off rebind whose
/// buffer content never flowed through the new row's journal — must
/// re-base the writer-side replica or the very next `AppendEdit` replays
/// buffer-coordinate edits against content of some other length and
/// recovery dies with an out-of-bounds replay. When the content the
/// returned steps' coordinates assume differs from `row_content`, one
/// synthetic replace-all bridge is computed and DEFERRED onto
/// `DocDb::pending_rebase` (its own doc comment: journaling it eagerly
/// would rewrite a reconstruction a never-edited re-baseline bind must
/// leave intact). The window steps replay verbatim only while they still
/// mirror the whole local journal; a journal that moved underneath them
/// (an undo inside the `Binding` window, or a window opened over an
/// existing journal) makes their coordinates unreplayable, so they are
/// subsumed into a single bridge to the live buffer instead — and THAT
/// bridge flushes eagerly, because the subsumed keystrokes exist nowhere
/// durable until it lands. The prior row's now-unreferenced shared
/// [`crate::db::FileBinding`] is pruned — a rebind must not leave the
/// abandoned row's baseline standing as a stale parallel source of truth.
fn bind_document_row(
    app: &mut App,
    id: DocumentId,
    mut doc_db: DocDb,
    row_content: &str,
    mode: BindMode,
) -> Vec<ReplicaStep> {
    let new_db_id = doc_db.db_id;
    let in_flight_appends = crate::db_enqueue::journal_i64(
        app.db_ops
            .values()
            .filter(|op| op.doc == id && op.is_append)
            .count(),
    );
    let Some(doc) = app.doc_mut(id) else {
        return Vec::new();
    };
    let prior = doc.doc_db_mut().map(|db| PriorBinding {
        db_id: db.db_id,
        last_known_seq: db.last_known_seq,
        pending_rebase: db.pending_rebase.take(),
        undo_offset: db.undo_offset,
        undo_floor: db.undo_floor,
        appends_sent: db.appends_sent,
    });
    let window = doc.replica.take_window();
    let mut pending = window.pending;
    let pos = crate::db_enqueue::journal_i64(doc.journal.pos());
    match prior {
        Some(prior) if prior.db_id == new_db_id => {
            match mode {
                BindMode::Preserved => {
                    doc_db.appends_sent = prior.appends_sent;
                    doc_db.undo_offset = prior.undo_offset;
                    doc_db.undo_floor = prior.undo_floor;
                }
                BindMode::Restarted { adopted } => {
                    doc_db.appends_sent = in_flight_appends;
                    doc_db.undo_offset = pos - in_flight_appends;
                    doc_db.undo_floor = if adopted { in_flight_appends + 1 } else { 1 };
                }
            }
            doc_db.last_known_seq = doc_db.last_known_seq.max(prior.last_known_seq);
            doc_db.pending_rebase = prior.pending_rebase;
            doc.replica = Replica::Bound(doc_db);
            pending
        }
        prior => {
            let window_intact = !pending.is_empty()
                && crate::db_enqueue::journal_i64(pending.len()) == pos
                && window.base.is_some();
            let flush_now = !pending.is_empty() && !window_intact;
            let base = match (window.base, window_intact) {
                (Some(base), true) => base,
                _ => {
                    pending.clear();
                    doc.buffer.content().to_string()
                }
            };
            let mut bridged = 0;
            if base != row_content {
                doc_db.pending_rebase = Some(ReplicaStep::new(
                    &[AppliedEdit {
                        start: 0,
                        end: row_content.len(),
                        deleted: row_content.to_string(),
                        insert: base,
                    }],
                    &[],
                    &[],
                ));
                doc_db.undo_floor = 1;
                bridged = 1;
            }
            let replayed = crate::db_enqueue::journal_i64(pending.len());
            doc_db.undo_offset = pos - replayed - bridged;
            doc.replica = Replica::Bound(doc_db);
            if let Some(prior) = prior {
                app.prune_file_binding(prior.db_id);
            }
            if flush_now {
                crate::db_enqueue::flush_pending_rebase(app, id);
            }
            pending
        }
    }
}

/// Drops `id`'s replica binding — `Detached`, dropping any buffered
/// `Binding` pending steps along with it — and prunes its now-possibly-
/// unreferenced shared [`crate::db::FileBinding`]. The shared exit both of
/// `handle_load_ack`'s refusal arms take (`saved_obs == None`, and
/// `Hydration::Refused`): a document a `Load` ack has just declined to
/// supply a trustworthy baseline for may never keep journaling against a
/// row this session no longer stands behind.
fn detach_file_binding(app: &mut App, id: DocumentId) {
    let old_db_id = app.doc_db_id(id);
    if let Some(doc) = app.doc_mut(id) {
        doc.replica = Replica::Detached;
    }
    if let Some(db_id) = old_db_id {
        app.prune_file_binding(db_id);
    }
}

/// Warns once that saving will detach the extra hard links `nlink` counts —
/// shared by `handle_load_ack` (every later reload) and `rune-cli`'s own
/// launch-time bootstrap (the session's very first `Load`), so the message
/// stays byte-identical between the two.
pub fn warn_hard_links(app: &mut App, nlink: i64) {
    if nlink > 1 {
        messages::warn(
            app,
            format!(
                "this file has {nlink} hard links \u{2014} saving replaces it atomically, so the other links keep the old content"
            ),
        );
    }
}

/// Records that `seq` was durably committed for `id`'s oldest still-pending
/// `AppendEdit` — called from `app::handle_db_event`'s `Msg::Db` handler on
/// `DbEvent::Ok { result: OpOutcome::Seq(seq), .. }`, after `app.db_ops` has
/// already resolved the ack's op id to `id`. `id` no longer live (an ack
/// racing a future close) is a correct, silent drop — the document it would
/// have updated is already gone.
pub fn resolve_append_ack(app: &mut App, id: DocumentId, seq: rune_db::Seq) {
    let Some(doc) = app.doc_mut(id) else { return };
    if let Some(doc_db) = doc.doc_db_mut() {
        doc_db.resolve_append_ack(seq);
    }
}

/// The reaction to a `CreateScratch` op's ack: `row_id` is a freshly
/// minted, never-bound scratch row — `id`'s
/// document binds to it exactly like a recovered launch-time scratch draft
/// does (`rune-cli::open::adopt_scratch_doc`), create-only because a
/// scratch row has never been bound to a real file, so its NEXT save must
/// still go through the create-only path. `id` no longer live (the draft
/// was closed while this op was still in flight) is a correct, silent
/// drop — `close_now` already sweeps `db_ops` of any entry pointing at a
/// closed document, but a race between that sweep and this ack landing is
/// still just a document that's gone, nothing to bind. Replays any
/// `Binding`-window `ReplicaStep`s the same way `install_doc_db` does — a
/// scratch draft can be typed into just as easily as an ordinary file
/// while its own `CreateScratch` is still in flight.
pub fn handle_create_scratch_ack(app: &mut App, id: DocumentId, row_id: i64) {
    bind_scratch_doc(app, id, row_id);
}

/// Binds `id` to the scratch row `row_id` through the one
/// [`bind_document_row`] chokepoint — shared by [`handle_create_scratch_ack`]
/// (the async mint) and [`adopt_scratch_doc`] (the launch-time synchronous
/// bind). A scratch row reconstructs to the empty string for this session,
/// and has never been bound to a real file, so it stays create-only.
pub fn bind_scratch_doc(app: &mut App, id: DocumentId, row_id: i64) {
    if app.doc(id).is_none() {
        return;
    }
    let pending = bind_document_row(
        app,
        id,
        DocDb::new(row_id, PublishMode::CreateOnly, rune_db::Seq(0)),
        "",
        BindMode::Restarted { adopted: false },
    );
    app.install_or_join_file_binding(row_id, None);
    crate::db_enqueue::replay_pending(app, id, pending);
}

/// The launch-time scratch adoption (`rune-cli`'s recovered-draft and
/// missing-path launches): binds `id` onto `row_id` and, when the store
/// recovered actual text, adopts it through
/// [`crate::document::Document::hydrate`] — the suspicion check, the
/// synthetic bridge `Step` so post-restart undo reaches the recovered text
/// in one step, and a refusal surfaced as a status rather than silently
/// applied.
pub fn adopt_scratch_doc(
    app: &mut App,
    id: DocumentId,
    row_id: i64,
    recovered: &str,
    journaled_cursors: &[rune_core::cursor::Cursor],
) {
    // Hydrated BEFORE binding: the bind chokepoint compares the buffer
    // against what the row reconstructs to (nothing, for a scratch row this
    // session has never journaled to) and computes the re-base bridge that
    // makes the adopted draft's coordinates replayable — bound first, the
    // adoption would land outside the row's lineage and the first edit
    // would journal coordinates the row cannot replay.
    if !recovered.is_empty()
        && let Some(doc) = app.doc_mut(id)
    {
        let disk_content = doc.buffer.content().to_string();
        if let crate::document::Hydration::Refused(reason) =
            doc.hydrate(&disk_content, recovered, journaled_cursors)
        {
            messages::error(app, format!("crash recovery: {reason}"));
        }
    }
    bind_scratch_doc(app, id, row_id);
}

/// The launch-time counterpart to [`install_doc_db`] for `rune-cli`'s
/// bootstrap `Load` (the initial positional): binds `id` onto an already
/// materialized `doc_db` through the one [`bind_document_row`] chokepoint,
/// AFTER the caller has run its own hydration — `row_content` is what the
/// bootstrap `Load` reported the row reconstructs to (`recovered`), so an
/// un-adopted divergence still gets its re-base bridge.
pub fn bind_loaded_doc(app: &mut App, id: DocumentId, doc_db: DocDb, row_content: &str) {
    let pending = bind_document_row(
        app,
        id,
        doc_db,
        row_content,
        BindMode::Restarted { adopted: false },
    );
    crate::db_enqueue::replay_pending(app, id, pending);
}

#[cfg(test)]
#[path = "db_ack_tests.rs"]
mod tests;
