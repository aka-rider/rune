//! The ack-reaction functions (split out of `materialize_ack.rs` for the
//! 500-line budget, plan WP2: the `quit_if_pending` addition pushed the
//! parent file over): what happens once a save/materialize attempt actually
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

/// Detects "a `bind_new` create lost the race": a concurrent writer's file
/// is already sitting at this document's own intended path when a create
/// was attempted. Returns that path — the caller's cue to hand the
/// document off to an ordinary `Load`, the only transition out of
/// `bind_new` that installs a real CAS baseline — for `handle_materialize_
/// ack` to route around the unanswerable `DiskConflict` Guard this refusal
/// would otherwise raise.
///
/// Only fires for a save of the document's OWN `file_path` (`doc.
/// pending_bind_path` is the discriminator: `bind_new_now`'s rename-create
/// route sets it and deliberately leaves `file_path` alone until the
/// publish commits, so `doc.file_path` would name a file this document has
/// never claimed — handing THAT off to a `Load` would ask `rune-db` to read
/// a path that has never existed). A naming attempt (`pending_bind_path`
/// set) keeps today's plain refusal instead: the racer's file sits at a
/// name this document has no claim on, so adopting its row would be wrong.
fn lost_create_race(app: &App, id: DocumentId, mat: &MatResult) -> Option<std::path::PathBuf> {
    mat.fresh.as_ref()?;
    let doc = app.doc(id)?;
    if !doc.db.as_ref().is_some_and(|d| d.bind_new) {
        return None;
    }
    if doc.pending_bind_path.is_some() {
        return None;
    }
    doc.file_path.clone()
}

/// The naming-attempt counterpart to [`lost_create_race`]: a `bind_new`
/// CREATE at a NEW target (`rename_create::bind_new_now`'s `^R` route)
/// losing the race. Never a `Load` hand-off — that is exactly what `lost_
/// create_race` above declines to do for this shape — and, just as
/// importantly, never the generic CAS-conflict `DiskConflict` Guard either:
/// there is no CAS baseline for a target this document has never claimed,
/// so `[S]ave anyway`/`[M]erge`/`[D]iscard` would all be dead ends. Mirrors
/// the no-store draft-create route's own `draft_collision_refusal` — a
/// footer refusal only.
fn naming_collision(app: &App, id: DocumentId, mat: &MatResult) -> Option<std::path::PathBuf> {
    mat.fresh.as_ref()?;
    let doc = app.doc(id)?;
    if !doc.db.as_ref().is_some_and(|d| d.bind_new) {
        return None;
    }
    doc.pending_bind_path.clone()
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
        // Once the bytes are published, the target exists, so the next save
        // is an overwrite — regardless of whether the bookkeeping that would
        // have supplied `saved` (and so a fresh CAS baseline) survived.
        if let Some(doc_db) = app.doc_mut(id).and_then(|d| d.db.as_mut()) {
            doc_db.bind_new = false;
        }
        match &mat.saved {
            Some(saved) => {
                if let Some(doc_db) = app.doc_mut(id).and_then(|d| d.db.as_mut()) {
                    doc_db.expect_obs = saved.id;
                }
            }
            None => {
                // A document may never be left with a store binding that
                // cannot serve its next save. The bookkeeping that would
                // have supplied a fresh CAS baseline was lost (`record_
                // outcome`'s synthetic-commit arms), so `bind_new: false`
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
                // kept dropped and only blocks saving kept standing.
                let path = app.doc(id).and_then(|d| d.file_path.clone());
                let store_usable = app.db.as_ref().is_some_and(|db| !db.degraded);
                match (path, store_usable) {
                    (Some(path), true) => crate::db_enqueue::load_document(app, id, &path),
                    _ => {
                        if let Some(doc) = app.doc_mut(id) {
                            doc.db = None;
                        }
                    }
                }
            }
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
        // Both discriminators read `pending_bind_path` — they must run
        // BEFORE the clear below wipes it. A refused rename-create, a
        // refused save of the document's own `file_path`, and an ordinary
        // CAS-conflict refusal all land here; `pending_bind_path` and
        // `bind_new` together are what tells the three apart.
        let race = lost_create_race(app, id, &mat);
        let naming = naming_collision(app, id, &mat);
        if let Some(doc) = app.doc_mut(id) {
            doc.abandon_save();
            // A refused create has nothing pending to bind — leaving it
            // standing would let a LATER, unrelated successful create
            // (`pending_bind_path.take()` above) bind this document to a
            // name it never wrote.
            doc.pending_bind_path = None;
        }
        if mat.missing {
            messages::error(app, "save failed: file no longer exists");
        } else if let Some(path) = race {
            // A create lost the race: `[S]ave anyway` would only re-run
            // `rename_excl` into the same EEXIST forever, and `[M]erge`/
            // `[D]iscard` would probe against the scratch row's `path=''`
            // and see nothing to merge — there is no CAS baseline to raise a
            // Guard against. Instead take the document through the same
            // transition an ordinary bound document already has:
            // `db_ack::handle_load_ack` installs `bind_new: false` with a
            // real baseline, even when it declines to hydrate because the
            // buffer moved on, so the user's typing is never clobbered. The
            // abandoned scratch row still holds everything typed so far and
            // will surface as a recoverable draft on the next bare launch —
            // that is correct, it is genuinely unsaved work at this instant.
            //
            // The hand-off is safe only when no OTHER live document in this
            // session is already bound to `path` — `handle_load_ack`'s own
            // precondition is that it is installing a FRESH binding, and a
            // row already carrying this-session history from that other
            // document could make `hydrate` replace this buffer's typing
            // instead of just anchoring a baseline. It is also safe only
            // with a usable store — a degraded/absent one would leave
            // `load_document` a silent no-op, `bind_new` stuck `true`
            // forever. Either way out, the racer's file itself is left
            // untouched: a direct-vfs fallback here would clobber a foreign
            // file this session has never observed.
            let hand_off_safe = !app.documents.iter().any(|(other_id, other)| {
                *other_id != id && other.file_path.as_deref() == Some(path.as_path())
            });
            let can_hand_off = hand_off_safe && app.db.as_ref().is_some_and(|db| !db.degraded);
            if can_hand_off {
                messages::error(
                    app,
                    "save failed: the target was created by something else; your changes are unsaved",
                );
                // A save-time refusal IS fresh evidence the disk moved —
                // seed it conservatively (`Diverged` is the superset of
                // what this refusal can mean) so the merge route this
                // hand-off is meant to reach is genuinely reachable, the
                // same way the CAS-conflict arm below already seeds it.
                if let Some(doc) = app.doc_mut(id) {
                    doc.last_sync = Some(rune_db::SyncKind::Diverged);
                }
                crate::db_enqueue::load_document(app, id, &path);
            } else {
                messages::error(
                    app,
                    "save failed: the target was created by something else; your buffer is intact \u{2014} ^R to a different name to save it",
                );
            }
        } else if let Some(target) = naming {
            // A rename-create losing the race at the NEW target: a footer
            // refusal only, matching the no-store draft-create route's own
            // `draft_collision_refusal` — never a Guard, since there is no
            // CAS baseline for a target this document has never claimed.
            let name = target
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| target.display().to_string());
            messages::error(app, format!("{name} already exists"));
        } else {
            messages::error(
                app,
                "save refused \u{2014} the file changed on disk since it was opened",
            );
            // Plan WP6.S4: a genuine CAS conflict — the fresh disk
            // observation `record_fresh_from_stat` already recorded — offers
            // the disk-conflict Guard so the user can act on it directly
            // rather than needing to know `^M` exists. A refused raise (a
            // Guard already up) leaves the message just posted above as the
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
/// append-only, so there is nothing to clear).
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
/// aborts the whole intent instead, regardless of version: never exit over
/// a save the user believes succeeded, and a wedged
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db::{Db, DbBridge, DocDb};
    use rune_core::buffer::Buffer;
    use rune_db::{ClockFn, Store};
    use rune_vfs::{Mem, Vfs};
    use std::sync::Arc;

    fn in_memory_db() -> Db {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let store = Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
        let bridge = DbBridge::bootstrap();
        Db::new(store, bridge, false)
    }

    /// A1 regression, re-shaped by blocker 3's re-baseline fix:
    /// `record_outcome`'s two synthetic-commit arms (a vanished store, and a
    /// `materialize_record` enqueue failure after the bytes already
    /// published) build `MatResult { committed: true, ..Default::default()
    /// }` — `saved: None`. With NO store at all to re-baseline from, a
    /// document may never be left with a binding that cannot serve its next
    /// save: `bind_new: false` paired with a stale `expect_obs` would make
    /// the very next save's `materialize_prepare` immediately `NotFound`.
    /// The correct outcome is dropping the binding entirely, so the next
    /// save falls back to the always-working direct-vfs path.
    #[test]
    fn a_synthesized_commit_with_no_saved_observation_and_no_store_drops_the_binding() {
        let mut app = App::new(
            Buffer::new("body"),
            Some(std::path::PathBuf::from("/root/nope.md")),
            Arc::new(Mem::new()),
            None,
        );
        let id = app.active;
        app.doc_mut(id).unwrap().db = Some(DocDb::new(1, 0, true, 0));

        handle_materialize_ack(
            &mut app,
            id,
            MatResult {
                committed: true,
                ..Default::default()
            },
        );

        assert!(
            app.doc(id).unwrap().db.is_none(),
            "a binding that can never serve its next save must be dropped, not left dangling"
        );
    }

    /// The same synthetic-commit shape, but with a live, non-degraded store
    /// still available: the re-baseline must go through an ordinary `Load`
    /// of the document's own (just-published) path rather than dropping the
    /// binding — the store CAN still serve a save, it just needs a fresh
    /// `expect_obs` to do it with.
    #[test]
    fn a_synthesized_commit_with_no_saved_observation_and_a_live_store_re_baselines_via_load() {
        let mut app = App::new(
            Buffer::new("body"),
            Some(std::path::PathBuf::from("/root/nope.md")),
            Arc::new(Mem::new()),
            Some(in_memory_db()),
        );
        let id = app.active;
        app.doc_mut(id).unwrap().db = Some(DocDb::new(1, 0, true, 0));

        handle_materialize_ack(
            &mut app,
            id,
            MatResult {
                committed: true,
                ..Default::default()
            },
        );

        assert!(
            app.doc(id).unwrap().db.is_some(),
            "a live store must keep the binding, re-baselined via Load, not drop it"
        );
        assert!(
            app.db_ops
                .values()
                .any(|p| p.doc == id && p.issued_version.is_some()),
            "a Load must be enqueued to install a fresh CAS baseline"
        );
    }
}
