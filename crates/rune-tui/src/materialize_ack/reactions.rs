//! The ack-reaction functions: what happens once a save/materialize attempt
//! actually resolves — `handle_materialize_ack`/`handle_save_done`'s
//! success/failure arms, the close-on-save-ack and quit-save-fan-out
//! continuations every one of them funnels through, and a local
//! (non-`rune-db`) materialize failure. The parent module keeps everything
//! upstream of the first reply and the dirty-cache chokepoint itself.

use rune_db::MatResult;

use crate::app::App;
use crate::document::DocumentId;
use crate::messages;
use crate::runtime::{CmdError, Effects};
use crate::workspace;

use super::recompute_dirty;

#[path = "committed.rs"]
mod committed;
use committed::handle_committed_ack;

/// A local (non-`rune-db`) materialize failure: a genuine `vfs` I/O error
/// on the caller-side write, or a store having vanished entirely
/// mid-flight. Fails only `id`'s save — never the whole store — since the
/// write's own failure carries no `rune-db` signal at all.
pub(crate) fn fail_materialize_locally(app: &mut App, id: DocumentId, message: impl Into<String>) {
    let pending_version = app
        .doc(id)
        .and_then(super::super::document::Document::pending_save_version);
    if let Some(doc) = app.doc_mut(id) {
        doc.abandon_save();
    }
    messages::error(app, message.into());
    recompute_dirty(app, id);
    resolve_continuations(app, id, pending_version, false);
}

/// Detects "a create-only publish lost the race": a concurrent writer's file
/// is already sitting at this document's own intended path when a create
/// was attempted. Returns that path — the caller's cue to hand the
/// document off to an ordinary `Load`, the only transition out of
/// `PublishMode::CreateOnly` that installs a real CAS baseline — for `handle_materialize_
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
fn lost_create_race(app: &App, id: DocumentId) -> Option<std::path::PathBuf> {
    let doc = app.doc(id)?;
    if !doc
        .doc_db()
        .is_some_and(|d| d.publish_mode.is_create_only())
    {
        return None;
    }
    if doc.bind_target().is_some() {
        return None;
    }
    doc.file_path.clone()
}

/// The naming-attempt counterpart to [`lost_create_race`]: a create-only
/// publish at a NEW target (`rename_create::bind_new_now`'s `^R` route)
/// losing the race. Never a `Load` hand-off — that is exactly what `lost_
/// create_race` above declines to do for this shape — and, just as
/// importantly, never the generic CAS-conflict `DiskConflict` Guard either:
/// there is no CAS baseline for a target this document has never claimed,
/// so `[S]ave anyway`/`[M]erge`/`[D]iscard` would all be dead ends. Mirrors
/// the no-store draft-create route's own `draft_collision_refusal` — a
/// footer refusal only.
fn naming_collision(app: &App, id: DocumentId) -> Option<std::path::PathBuf> {
    let doc = app.doc(id)?;
    if !doc
        .doc_db()
        .is_some_and(|d| d.publish_mode.is_create_only())
    {
        return None;
    }
    doc.bind_target().cloned()
}

/// The reaction to a `materialize` ack for `id` (plan WP5.S6, re-shaped by
/// WP7's `MaterializeRecord`): advances `saved_version`/`DocDb::expect_obs`/
/// `publish_mode` on a commit, surfaces each `MatResult` outcome as status text,
/// and — either way — clears `id`'s `save_in_flight` and recomputes its
/// dirty cache (trigger (b) of `recompute_dirty`'s doc comment). Also
/// called synthetically (WP7, `MatResult::Committed { saved: None }`) when
/// the disk write physically succeeded but the DB-side bookkeeping that
/// would have supplied `saved` was lost to a dead writer — see
/// `record_outcome`'s doc comment.
pub(crate) fn handle_materialize_ack(app: &mut App, id: DocumentId, mat: &MatResult) {
    let Some(doc) = app.doc(id) else { return };
    // `MatResult` carries no version of its own — this peek is how the
    // chokepoint below correlates the ack against the SAME bytes
    // `begin_save` captured, never a later unrelated capture (plan WP1),
    // and is also `quit_if_pending`'s (plan WP2) own correlation key.
    let pending_version = doc.pending_save_version();
    let committed = matches!(
        mat,
        MatResult::Committed { .. } | MatResult::CommittedRaced { .. }
    );

    match mat {
        MatResult::Committed { saved } => {
            handle_committed_ack(app, id, pending_version, saved.as_ref(), false);
        }
        MatResult::CommittedRaced { saved, .. } => {
            handle_committed_ack(app, id, pending_version, Some(saved), true);
        }
        MatResult::Missing => {
            if let Some(doc) = app.doc_mut(id) {
                doc.abandon_save();
            }
            messages::error(app, "save failed: file no longer exists");
        }
        MatResult::Refused { .. } => {
            handle_refused_ack(app, id);
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
    resolve_continuations(app, id, pending_version, committed);
}

fn handle_refused_ack(app: &mut App, id: DocumentId) {
    // Both discriminators read `bind_target` — they must run BEFORE
    // `abandon_save` below wipes it (it now lives inside `save` itself,
    // so a single resolve clears it atomically — no separate "was this
    // ack actually still current" re-check needed the way a bare field
    // once required). A refused rename-create, a refused save of the
    // document's own `file_path`, and an ordinary CAS-conflict refusal
    // all land here; `bind_target` and `publish_mode` together are what
    // tells the three apart.
    let race = lost_create_race(app, id);
    let naming = naming_collision(app, id);
    if let Some(doc) = app.doc_mut(id) {
        doc.abandon_save();
    }
    if let Some(path) = race {
        // A create lost the race: `[S]ave anyway` would only re-run
        // `rename_excl` into the same EEXIST forever, and `[M]erge`/
        // `[D]iscard` would probe against the scratch row's `path=''`
        // and see nothing to merge — there is no CAS baseline to raise a
        // Guard against. Instead take the document through the same
        // transition an ordinary bound document already has:
        // `db_ack::handle_load_ack` installs the overwrite mode with a
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
        // no-op, the document stuck create-only forever. Either way out, the
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
                    && other.file_path.as_deref().is_some_and(|p| {
                        workspace::resolve(app.vfs.as_ref(), p)
                            .map_or(true, |other_resolved| other_resolved == resolved_path)
                    })
            }),
            Err(_) => false,
        };
        let can_hand_off = hand_off_safe && app.db.as_ref().is_some_and(|db| !db.degraded);
        if can_hand_off {
            messages::error(
                app,
                "save failed: the target was created by something else; your changes are unsaved \u{2014} ^M to merge",
            );
            // `Diverged` is the superset of what this refusal can mean:
            // the racer's file proves only that something else wrote the
            // target, never how the two sides relate.
            super::seed_refusal_classification(app, id, rune_db::SyncKind::Diverged);
            let _ = crate::db_enqueue::load_document(
                app,
                id,
                &path,
                crate::db_enqueue::LoadIntent::Rebaseline,
            );
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
        let name = target.file_name().map_or_else(
            || target.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        messages::error(app, format!("{name} already exists"));
        // Mirrors `draft_collision_refusal`'s own pairing with
        // `return_to_title`: without it the Editor keeps focus while
        // the title bar still shows the refused name, leaving the user
        // with no direct way back into the field to retype it.
        app.refocus_title();
    } else {
        messages::error(app, super::SAVE_REFUSED_DISK_CHANGED);
        // Plan WP6.S4: a genuine CAS conflict — the fresh disk
        // observation `record_fresh_from_stat` already recorded — offers
        // the disk-conflict Guard so the user can act on it directly
        // rather than needing to know `^M` exists. `Diverged` is the
        // superset of what a CAS refusal can mean: the comparison itself
        // proves only that disk moved, never how the two sides relate.
        super::raise_disk_conflict(app, id, rune_db::SyncKind::Diverged);
    }
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
    result: Result<(), CmdError>,
    durable: bool,
) {
    if app
        .doc(id)
        .and_then(super::super::document::Document::save_ticket)
        != Some(ticket)
    {
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
    resolve_continuations(app, id, Some(version), succeeded);
}

/// Every way a save attempt can end — committed, refused, abandoned before
/// it ever reached disk — funnels through here, so a continuation waiting
/// on that save (a `^w` close, a quit fan-out) is answered exactly once and
/// can never outlive the attempt that armed it.
pub(super) fn resolve_continuations(
    app: &mut App,
    id: DocumentId,
    version: Option<u64>,
    succeeded: bool,
) {
    close_if_pending(app, id, succeeded);
    quit_if_pending(app, id, version, succeeded);
}

/// Only closes when `id` is STILL the document `pending_close_on_save` names —
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

/// Retires `id`'s entry in `App::quit`'s `SaveFanOut` iff
/// `version` matches what THIS entry recorded (idempotent — a later,
/// unrelated ack for the same document, or a duplicate delivery, can never
/// retire an entry twice or retire the wrong capture) and, if the map is
/// now empty, completes the quit (`should_quit = true`). A FAILED save
/// aborts the whole intent instead, regardless of version: never exit over
/// a save the user believes succeeded, and a wedged
/// continuation waiting on a save that will never retry is worse than
/// telling the user plainly that quit did not happen.
///
/// A no-op when `id` isn't in `App::quit`'s `SaveFanOut` at all — every OTHER
/// document's save ack, and every ack once no quit-save fan-out is
/// outstanding, must never touch this state.
fn quit_if_pending(app: &mut App, id: DocumentId, version: Option<u64>, succeeded: bool) {
    let Some(intent) = app.quit.fan_out() else {
        return;
    };
    if !intent.pending.contains_key(&id) {
        return;
    }
    if succeeded {
        if version == app.quit.fan_out().and_then(|i| i.pending.get(&id).copied()) {
            retire_quit_wait(app, id);
        }
    } else {
        app.quit = crate::app::QuitNegotiation::Idle;
    }
}

/// Removes `id` from an outstanding `App::quit`'s `SaveFanOut` wait set — called
/// by `quit_if_pending` above on a matching successful ack, and by
/// `workspace::close_now` (plan WP2) when the document a quit-save was
/// waiting on gets closed out from under it instead (a `[D]iscard` on a
/// SEPARATE Guard, say): either way, quit no longer has anything left to
/// wait on FROM THIS document. Completes the quit the same way a
/// successful ack would once the wait set empties out entirely — a close
/// is exactly as final an answer as a successful save for the purpose of
/// "is there still unpersisted work quit needs to wait on".
pub(crate) fn retire_quit_wait(app: &mut App, id: DocumentId) {
    let Some(intent) = app.quit.fan_out_mut() else {
        return;
    };
    if intent.pending.remove(&id).is_none() {
        return;
    }
    if intent.pending.is_empty() {
        app.quit = crate::app::QuitNegotiation::Idle;
        app.should_quit = true;
    }
}

#[cfg(test)]
#[path = "reactions_tests.rs"]
mod tests;
