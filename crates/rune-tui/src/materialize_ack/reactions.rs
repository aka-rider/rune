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
/// bind_target` is the discriminator: `bind_new_now`'s rename-create route
/// sets it and deliberately leaves `file_path` alone until the publish
/// commits, so `doc.file_path` would name a file this document has never
/// claimed — handing THAT off to a `Load` would ask `rune-db` to read a
/// path that has never existed). A naming attempt (`bind_target` set) keeps
/// today's plain refusal instead: the racer's file sits at a name this
/// document has no claim on, so adopting its row would be wrong.
fn lost_create_race(app: &App, id: DocumentId, mat: &MatResult) -> Option<std::path::PathBuf> {
    mat.fresh.as_ref()?;
    let doc = app.doc(id)?;
    if !doc.doc_db().is_some_and(|d| d.bind_new) {
        return None;
    }
    if doc.bind_target().is_some() {
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
    if !doc.doc_db().is_some_and(|d| d.bind_new) {
        return None;
    }
    doc.bind_target().cloned()
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
        if let Some(path) = app.doc_mut(id).and_then(|d| d.take_bind_target()) {
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
        // `bind_new` stays on THIS document's own `DocDb` (never shared — a
        // scratch row racing to bind is claimed by exactly one document),
        // but the epoch/baseline below live on the SHARED `FileBinding` for
        // `db_id`, so every OTHER tab open on the same file sees the same
        // advance a single tab's own save just produced — closing the
        // false-conflict class where a second tab's next save compares
        // against a stale per-tab copy that never learned about this commit,
        // instead of the file's true current baseline.
        let db_id = app.doc_db_id(id);
        if let Some(doc_db) = app.doc_mut(id).and_then(|d| d.doc_db_mut()) {
            doc_db.bind_new = false;
        }
        if let Some(binding) = app.doc_file_binding_mut(id) {
            // The publish just committed — any `Probe` issued before this
            // reply lands, by ANY document bound to `db_id`, is now
            // describing a disk that no longer exists; bumping the epoch
            // here is what makes `db_dispatch`'s `OpOutcome::Sync` arm drop
            // such a stale reply instead of trusting it.
            binding.save_epoch = binding.save_epoch.wrapping_add(1);
        }
        if let Some(saved) = &mat.saved
            && let Some(binding) = app.doc_file_binding_mut(id)
        {
            binding.expect_obs = saved.id;
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
            doc.nlink = mat.saved.as_ref().and_then(|o| o.nlink);
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
        if mat.saved.is_none() {
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
            // kept dropped and only blocks saving kept standing. The
            // `Load` is `binding_only` (never a recovery adoption) — this
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
        if mat.raced {
            messages::info(
                app,
                "saved \u{2014} a concurrent external change was overwritten; its bytes were preserved",
            );
        }
    } else {
        // Both discriminators read `bind_target` — they must run BEFORE
        // `abandon_save` below wipes it (it now lives inside `save` itself,
        // so a single resolve clears it atomically — no separate "was this
        // ack actually still current" re-check needed the way a bare field
        // once required). A refused rename-create, a refused save of the
        // document's own `file_path`, and an ordinary CAS-conflict refusal
        // all land here; `bind_target` and `bind_new` together are what
        // tells the three apart.
        let race = lost_create_race(app, id, &mat);
        let naming = naming_collision(app, id, &mat);
        if let Some(doc) = app.doc_mut(id) {
            doc.abandon_save();
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
            // instead of just anchoring a baseline. Paths are compared
            // RESOLVED (`workspace::resolve`, the one chokepoint every
            // path that binds a document funnels through) — a launch
            // positional can still be relative while an Explorer-opened
            // tab holds the absolute spelling of the very same file, and
            // an unresolved comparison would miss that they name one
            // document. It is also safe only with a usable store — a
            // degraded/absent one would leave `load_document` a silent
            // no-op, `bind_new` stuck `true` forever. Either way out, the
            // racer's file itself is left untouched: a direct-vfs fallback
            // here would clobber a foreign file this session has never
            // observed.
            // Either `resolve` call failing here — this document's own
            // target, or some other open document's own binding — leaves
            // no proof the two name different files, so it takes the
            // conservative branch below (`hand_off_safe = false`) exactly
            // like a genuine collision would, rather than risking a hand-
            // off `hydrate` could clobber.
            let hand_off_safe = match workspace::resolve(app.vfs.as_ref(), &path) {
                Ok(resolved_path) => !app.documents.iter().any(|(other_id, other)| {
                    *other_id != id
                        && match other.file_path.as_deref() {
                            Some(p) => match workspace::resolve(app.vfs.as_ref(), p) {
                                Ok(other_resolved) => other_resolved == resolved_path,
                                Err(_) => true,
                            },
                            None => false,
                        }
                }),
                Err(_) => false,
            };
            let can_hand_off = hand_off_safe && app.db.as_ref().is_some_and(|db| !db.degraded);
            if can_hand_off {
                messages::error(
                    app,
                    "save failed: the target was created by something else; your changes are unsaved \u{2014} ^M to merge",
                );
                // A save-time refusal IS fresh evidence the disk moved —
                // seed it conservatively (`Diverged` is the superset of
                // what this refusal can mean) so the merge route this
                // hand-off is meant to reach is genuinely reachable, the
                // same way the CAS-conflict arm below already seeds it.
                // Without it, the Load this hand-off enqueues installs a
                // fresh CAS baseline straight off the racer's own current
                // bytes, and a later plain ⌘S would then CAS-match and
                // silently overwrite the racer instead of leaving `^M`
                // reachable as the way to reconcile the two.
                if let Some(doc) = app.doc_mut(id) {
                    doc.last_sync = Some(rune_db::SyncKind::Diverged);
                }
                let _ = crate::db_enqueue::load_document(app, id, &path, true);
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
            // Mirrors `draft_collision_refusal`'s own pairing with
            // `return_to_title`: without it the Editor keeps focus while
            // the title bar still shows the refused name, leaving the user
            // with no direct way back into the field to retype it.
            app.refocus_title();
        } else {
            messages::error(
                app,
                "save refused \u{2014} the file changed on disk since it was opened",
            );
            // Plan WP6.S4: a genuine CAS conflict — the fresh disk
            // observation `record_fresh_from_stat` already recorded — offers
            // the disk-conflict Guard so the user can act on it directly
            // rather than needing to know `^M` exists.
            if mat.fresh.is_some() {
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
                let _ = guard::set_guard_or_warn(
                    app,
                    GuardPrompt {
                        doc: id,
                        kind: GuardKind::DiskConflict,
                    },
                    "disk-conflict confirmation dropped \u{2014} a prompt is already showing",
                );
            }
        }
    }
    // The save this document's `save_in_flight` gated is resolved either
    // way now — a `Probe` deferred against `db_id` (`db_enqueue::probe`'s
    // own doc comment) can finally read the post-save world exactly once.
    // The deferral flag lives on the SHARED `FileBinding`, not this one
    // document's own `DocDb`: ANY save on this file resolving is what
    // unblocks it, and once it fires, EVERY document still open on this
    // file gets its own fresh probe — not just `id`, whose save happened to
    // be the one that resolved.
    let db_id = app.doc_db_id(id);
    let deferred_probe = app
        .doc_file_binding_mut(id)
        .is_some_and(|binding| std::mem::take(&mut binding.pending_probe));
    if deferred_probe && let Some(db_id) = db_id {
        for doc_id in app.documents_bound_to(db_id) {
            crate::db_enqueue::probe(app, doc_id);
        }
    }
    recompute_dirty(app, id);
    close_if_pending(app, id, mat.committed);
    quit_if_pending(app, id, pending_version, mat.committed);
}

/// The reaction to `Msg::SaveDone` — the no-store fallback save path's own
/// completion, or a leftover reply for a document whose store binding
/// vanished mid-flight. `ticket` is checked against the document's own
/// `save_ticket` first — a stale reply for an attempt this document has
/// already moved on from (or a document that has since closed) is a typed,
/// silent drop. Success posts only the durability warning, and only when
/// the write could not be confirmed durable.
pub(crate) fn handle_save_done(
    app: &mut App,
    id: DocumentId,
    ticket: crate::document::SaveTicket,
    version: u64,
    result: Result<(), String>,
    durable: bool,
) {
    if app.doc(id).and_then(|d| d.save_ticket()) != Some(ticket) {
        return;
    }
    let succeeded = result.is_ok();
    match result {
        Ok(()) => {
            if let Some(doc) = app.doc_mut(id) {
                doc.finish_save_ok(version);
            }
            if !durable {
                messages::warn(app, super::DURABILITY_UNCONFIRMED_WARNING);
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
#[path = "reactions_tests.rs"]
mod tests;
