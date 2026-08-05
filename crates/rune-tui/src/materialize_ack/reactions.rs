//! The ack-reaction functions (split out of `materialize_ack.rs` for the
//! §1.6 budget, plan WP2: the `quit_if_pending` addition pushed the parent
//! file over): what happens once a save/materialize attempt actually
//! resolves — `handle_materialize_ack`/`handle_save_done`'s success/failure
//! arms, the close-on-save-ack and quit-save-fan-out chokepoints they both
//! funnel through, and a local (non-`rune-db`) materialize failure. The
//! parent module keeps everything upstream of the first reply (`handle_
//! prepare_ack`/`handle_materialize_vfs_done`/`record_outcome`/`on_store_
//! failure`) and the dirty-cache chokepoint itself.

use rune_db::MatResult;

use crate::app::App;
use crate::document::DocumentId;
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::messages;
use crate::runtime::Effects;
use crate::workspace;

use super::recompute_dirty;

/// A local (non-`rune-db`) materialize failure: a genuine `vfs` I/O error
/// on the caller-side write, or a store having vanished entirely
/// mid-flight. Fails only `id`'s save — never the whole store — since the
/// write's own failure carries no `rune-db` signal at all.
pub(crate) fn fail_materialize_locally(app: &mut App, id: DocumentId, message: impl Into<String>) {
    let pending_version = app.doc(id).and_then(|d| d.pending_save_version());
    if let Some(doc) = app.doc_mut(id) {
        doc.abandon_save();
        doc.pending_bind_path = None;
    }
    messages::error(app, message.into());
    recompute_dirty(app, id);
    quit_if_pending(app, id, pending_version, false);
}

/// The reaction to a `materialize` ack for `id` (plan WP5.S6, re-shaped by
/// WP7's `MaterializeRecord`): advances `saved_version`/`DocDb::expect_obs`/
/// `bind_new` on a commit, surfaces each `MatResult` outcome as status text,
/// and — either way — clears `id`'s `save_in_flight` and recomputes its
/// dirty cache (trigger (b) of `recompute_dirty`'s doc comment). Also
/// called synthetically (WP7, a `committed: true`, otherwise-default
/// `MatResult`) when the disk write physically succeeded but the DB-side
/// bookkeeping that would have supplied `saved`/`raced` was lost to a dead
/// writer — see `record_outcome`'s doc comment.
pub(crate) fn handle_materialize_ack(app: &mut App, id: DocumentId, mat: MatResult) {
    let Some(doc) = app.doc(id) else { return };
    // `MatResult` carries no version of its own — this peek is how the
    // chokepoint below correlates the ack against the SAME bytes
    // `begin_save` captured, never a later unrelated capture (plan WP1),
    // and is also `quit_if_pending`'s (plan WP2) own correlation key.
    let pending_version = doc.pending_save_version();

    if mat.committed {
        // A committed bind-new create is where an untitled draft finally
        // gets its path — only now, after the no-clobber publish actually
        // succeeded (see `bind_new_now`'s docs).
        if let Some(path) = app.doc_mut(id).and_then(|d| d.pending_bind_path.take()) {
            if let Some(doc) = app.doc_mut(id) {
                doc.bind_path(path);
            }
            if app.active == id {
                let name = app.doc(id).map(crate::title::name_for).unwrap_or_default();
                app.title.seed(&name);
            }
        }
        if let Some(saved) = &mat.saved
            && let Some(doc_db) = app.doc_mut(id).and_then(|d| d.db.as_mut())
        {
            doc_db.expect_obs = saved.id;
            doc_db.bind_new = false;
        }
        if let Some(doc) = app.doc_mut(id) {
            match pending_version {
                Some(version) => {
                    doc.finish_save_ok(version);
                }
                None => doc.abandon_save(),
            }
        }
        if mat.raced {
            messages::info(
                app,
                "saved \u{2014} a concurrent external change was overwritten; its bytes were preserved",
            );
        }
    } else {
        if let Some(doc) = app.doc_mut(id) {
            doc.abandon_save();
        }
        if mat.missing {
            messages::error(app, "save failed: file no longer exists");
        } else {
            messages::error(
                app,
                "save refused \u{2014} the file changed on disk since it was opened",
            );
            // Plan WP6.S4: a genuine CAS conflict — the fresh disk
            // observation `record_fresh_from_stat` already recorded — offers
            // the disk-conflict Guard so the user can act on it directly
            // rather than needing to know `⌘M` exists. A refused raise (a
            // Guard already up) leaves the plain status line above as the
            // only feedback, which is still correct — `set_guard`'s
            // `#[must_use]` return says exactly that happened.
            if let Some(fresh) = &mat.fresh {
                // `merge::begin`'s own fast pre-check (plan Gotchas `[R3]`)
                // reads `last_sync` as a hint only — this CAS refusal IS
                // fresh evidence the disk moved, so seed it conservatively
                // (`Diverged` is the superset of what a save-time refusal
                // can mean) rather than leaving `[M]erge`/`[D]iscard` here
                // refused on a stale `Clean` from the last probe/load. The
                // AUTHORITATIVE classification still happens fresh inside
                // the `MergePrep` landing either answer starts.
                if let Some(doc) = app.doc_mut(id) {
                    doc.last_sync = Some(rune_db::SyncKind::Diverged);
                }
                let _ = guard::set_guard(
                    app,
                    GuardPrompt {
                        doc: id,
                        kind: GuardKind::DiskConflict {
                            fresh_obs: fresh.id,
                        },
                    },
                );
            }
        }
    }
    recompute_dirty(app, id);
    close_if_pending(app, id, mat.committed);
    quit_if_pending(app, id, pending_version, mat.committed);
}

/// The reaction to `Msg::SaveDone` — the no-store fallback save path's own
/// completion (plan decision 5), or a leftover reply for a document whose
/// store binding vanished mid-flight. Success posts nothing (the log is
/// append-only, so there is nothing to clear — plan WP4.S2, superseding
/// review finding F2's provenance-aware clear).
pub(crate) fn handle_save_done(
    app: &mut App,
    id: DocumentId,
    version: u64,
    result: Result<(), String>,
) {
    let succeeded = result.is_ok();
    match result {
        Ok(()) => {
            if let Some(doc) = app.doc_mut(id) {
                doc.finish_save_ok(version);
            }
        }
        Err(e) => {
            if let Some(doc) = app.doc_mut(id) {
                doc.abandon_save();
            }
            messages::error(app, format!("save failed: {e}"));
        }
    }
    recompute_dirty(app, id);
    close_if_pending(app, id, succeeded);
    quit_if_pending(app, id, Some(version), succeeded);
}

/// The close-on-save-ack chokepoint (plan WP5.S3): both save completion
/// paths (`handle_materialize_ack`'s store-backed flow and this module's
/// own no-store `handle_save_done` fallback — Assumption A1 documents with
/// `db: None` take THIS path, never the other) funnel through here. Only
/// closes when `id` is STILL the document `pending_close_on_save` names —
/// a later unrelated `^w`/Guard interaction on a DIFFERENT document
/// overwrites that single global slot, which correctly abandons (never
/// mis-fires) this document's stale close intent — and only when the save
/// itself actually succeeded; a failed save leaves the document open with
/// its usual error surfaced instead of losing the user's only path back to
/// it.
fn close_if_pending(app: &mut App, id: DocumentId, succeeded: bool) {
    if app.pending_close_on_save != Some(id) {
        return;
    }
    app.pending_close_on_save = None;
    if succeeded {
        // A scratch sink, discarded — see `workspace::close::close_now`'s
        // own doc comment: this call chain never touches an image document.
        let mut effects = Effects::default();
        let _ = workspace::close_now(app, id, &mut effects);
    }
}

/// The quit-save fan-out's ack-side chokepoint (plan WP2): both save
/// completion paths funnel through here exactly like `close_if_pending`
/// above, and for the same reason — a document's own save ack is the only
/// place that can correctly resolve whatever OTHER continuation (close,
/// quit) was waiting on it. Retires `id`'s entry in `App::quit_intent` iff
/// `version` matches what THIS entry recorded (idempotent — a later,
/// unrelated ack for the same document, or a duplicate delivery, can never
/// retire an entry twice or retire the wrong capture) and, if the map is
/// now empty, completes the quit (`should_quit = true`). A FAILED save
/// aborts the whole intent instead, regardless of version — Go parity:
/// never exit over a save the user believes succeeded, and a wedged
/// continuation waiting on a save that will never retry is worse than
/// telling the user plainly that quit did not happen.
///
/// A no-op when `id` isn't in `App::quit_intent` at all — every OTHER
/// document's save ack, and every ack once no quit-save fan-out is
/// outstanding, must never touch this state.
pub(crate) fn quit_if_pending(
    app: &mut App,
    id: DocumentId,
    version: Option<u64>,
    succeeded: bool,
) {
    let Some(intent) = app.quit_intent.as_ref() else {
        return;
    };
    if !intent.pending.contains_key(&id) {
        return;
    }
    if succeeded {
        if version
            == app
                .quit_intent
                .as_ref()
                .and_then(|i| i.pending.get(&id).copied())
        {
            retire_quit_wait(app, id);
        }
    } else {
        app.quit_intent = None;
    }
}

/// Removes `id` from an outstanding `App::quit_intent`'s wait set — called
/// by `quit_if_pending` above on a matching successful ack, and by
/// `workspace::close_now` (plan WP2) when the document a quit-save was
/// waiting on gets closed out from under it instead (a `[D]iscard` on a
/// SEPARATE Guard, say): either way, quit no longer has anything left to
/// wait on FROM THIS document. Completes the quit the same way a
/// successful ack would once the wait set empties out entirely — a close
/// is exactly as final an answer as a successful save for the purpose of
/// "is there still unpersisted work quit needs to wait on".
pub(crate) fn retire_quit_wait(app: &mut App, id: DocumentId) {
    let Some(intent) = app.quit_intent.as_mut() else {
        return;
    };
    if intent.pending.remove(&id).is_none() {
        return;
    }
    if intent.pending.is_empty() {
        app.quit_intent = None;
        app.should_quit = true;
    }
}
