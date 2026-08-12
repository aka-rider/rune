//! The rename state machine — one `pub rename: RenameState` field on `App`.
//!
//! ⌃R (or Up at the top of the editor) focuses the title; Enter commits a
//! changed name; a destination that already exists raises a
//! `GuardKind::RenameCollision` prompt; and `[R]eplace` preserves the
//! replaced file's bytes as a durable blob before destroying it —
//! that blob being the only record the file ever existed, since rune never
//! opened it.
//!
//! ### Why a typed machine and not two booleans
//!
//! The existing confirmation machinery (`guard`'s `GuardPrompt`) is a
//! *prompt*: `handle_guard_key` resolves every dirty-close outcome
//! synchronously in one match arm. `[R]eplace` cannot work that way — it is
//! capture-displaced-bytes, *then* swap, *then* commit, *then* unlink, with
//! a mid-sequence point past which it is not cancellable. A pair of
//! booleans on `App` (following `pending_quit` / `pending_close_on_save` /
//! `pending_save_confirm`, all three already called out as ad hoc in their
//! own doc comments) would permit two states the capture-before-destroy
//! rule forbids: reaching a swap without having captured, and prompting
//! about a collision an external process already resolved.
//!
//! So the *prompt* stays in `guard`/`footer` — the component that renders
//! the feedback — and this machine drives the I/O.
//!
//! ### The states, and the ones that deliberately don't exist
//!
//! - **No `Replacing`.** Capture-then-swap is one non-cancellable `rune-db`
//!   op, so "swapped but not captured" is not representable across a
//!   message boundary.
//! - **No `Failed`.** Nothing would wait in it and nothing would leave it;
//!   the message log already owns errors. Every failure edge goes to `Idle`
//!   + `messages::error` + refocus the title with the typed name.
//! - **No `typed: String`.** The typed name IS `to.file_name()` — one
//!   value, one meaning.
//!
//! Single slot: a second commit while one is in flight is **refused**,
//! never queued.
//!
//! ### Global invariant
//!
//! `matches!(app.rename, RenameState::Collision { .. })` ⟺ a
//! `RenameCollision` Guard is up. `guard::set_guard` returning `bool`
//! guards the raise side; `guard::clear_guard` calling
//! [`on_prompt_dismissed`] guards the removal side.
//!
//! ### Known limitation
//!
//! `[R]eplace` requires a per-document `rune-db` binding to capture the
//! displaced bytes into. Today that is only the bootstrap
//! document: every Explorer-opened document has `db: None` until per-doc
//! hydration lands (`workspace::open_path`, TODO.md). For those, the
//! collision prompt offers no `[R]eplace` at all and the plain rename is
//! the only outcome. This is stated here rather than left to be discovered
//! in review.

use std::path::{Path, PathBuf};

use rune_db::RenameOutcome;
use rune_vfs::Stat;

use crate::app::App;
use crate::document::DocumentId;
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::messages;
use crate::runtime::Effects;
use crate::title;

/// Which reply route an in-flight rename is waiting on.
///
/// `Cmd(u32)` is the no-store path (a spawned `Cmd` replying with
/// `Msg::RenameDone`); `Db(u64)` is the `rune-db` writer-FIFO op id.
/// `spawn_cmd` has no cancellation, so a dismissed-then-restarted rename
/// would otherwise be corrupted by the first reply landing late — the
/// generation echo mirrors `load_dir_cmd`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ticket {
    Cmd(u32),
    Db(u64),
}

/// The rename workflow's state. `Idle` is the default and the destination
/// of every terminal edge.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum RenameState {
    #[default]
    Idle,
    /// A no-clobber rename is in flight.
    Committing {
        doc: DocumentId,
        from: PathBuf,
        to: PathBuf,
        ticket: Ticket,
    },
    /// The destination exists and the user is being asked. `seen` is the
    /// destination's stat at the moment of the collision — the **consent**
    /// baseline ("still the file you agreed to replace?"), not the safety
    /// mechanism; safety comes from `rune-db`'s post-swap capture.
    Collision {
        doc: DocumentId,
        from: PathBuf,
        to: PathBuf,
        seen: Stat,
    },
    /// The confirmed destructive replace is in flight.
    Capturing {
        doc: DocumentId,
        from: PathBuf,
        to: PathBuf,
        seen: Stat,
        ticket: Ticket,
    },
}

impl RenameState {
    fn ticket(&self) -> Option<Ticket> {
        match self {
            RenameState::Committing { ticket, .. } | RenameState::Capturing { ticket, .. } => {
                Some(*ticket)
            }
            RenameState::Idle | RenameState::Collision { .. } => None,
        }
    }

    fn doc(&self) -> Option<DocumentId> {
        match self {
            RenameState::Committing { doc, .. }
            | RenameState::Collision { doc, .. }
            | RenameState::Capturing { doc, .. } => Some(*doc),
            RenameState::Idle => None,
        }
    }

    /// Whether a rename op is outstanding. `pub(crate)` so `save` can
    /// refuse from its side too: the two refusals are a symmetric pair,
    /// since either operation started while the other is mid-flight would
    /// act on a path the other is about to invalidate.
    pub(crate) fn in_flight(&self) -> bool {
        self.ticket().is_some()
    }
}

/// Whether the title may release focus. `Refused` keeps the user in the
/// field with the reason already in the footer (decision 7); the caller is
/// never blocked from proceeding regardless of which variant comes back —
/// only the FOCUS transition is vetoed, never the command that asked for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    Accepted,
    Refused,
}

/// Whether `[R]eplace` can be offered: it needs a durable store to capture
/// the displaced bytes into BEFORE they are destroyed. A
/// degraded store still counts — it is a live, if untrusted, connection,
/// and a `put_blob` that fails there surfaces as an ordinary `Err` edge
/// rather than a silent loss.
pub fn replace_allowed(app: &App) -> bool {
    let Some(doc_id) = collision_doc(app) else {
        return false;
    };
    app.db.is_some() && app.doc(doc_id).is_some_and(|d| d.is_store_bound())
}

fn collision_doc(app: &App) -> Option<DocumentId> {
    match &app.rename {
        RenameState::Collision { doc, .. } => Some(*doc),
        _ => None,
    }
}

/// The destination the collision prompt is about, for the footer's label.
pub fn collision_target(app: &App) -> Option<String> {
    match &app.rename {
        RenameState::Collision { to, .. } => Some(display_name(to)),
        _ => None,
    }
}

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Starts a rename of `app.active` to the title field's typed name.
///
/// Returns whether the title may now release focus (`Commit`) — NOT whether
/// a rename was actually started: the draft-create route, an unchanged
/// name, a successful enqueue, and an enqueue failure that has already been
/// reported through `on_store_failure` all return `Accepted`, because none
/// of them leaves the user needing to fix anything in the field. Every
/// `Refused` path's SOLE side effect is posting a message to the log —
/// that is what makes a refused blur running this twice in
/// one keystroke
/// harmless, since the second run just re-reports the same status. Every
/// refusal below leaves the state `Idle`, the buffer byte-identical,
/// `file_path` unchanged, and the journal untouched — a refused rename must
/// be indistinguishable from never having asked. Focus itself is never
/// written here: the caller (`App::set_focus`) decides where it lands once
/// it has this verdict.
pub fn begin(app: &mut App, effects: &mut Effects) -> Commit {
    // A second commit while one is in flight is refused, never queued: the
    // in-flight op captured its own `from`, and letting a second one race
    // it would mean two ops disagreeing about where the file is.
    if app.rename.in_flight() {
        messages::warn(app, "a rename is already in progress");
        return Commit::Refused;
    }

    let id = app.active;
    // No status is set on this branch — `app.active` naming a document that
    // isn't there is unreachable in practice, and `Refused` here would trap
    // the user in the title with an empty footer and no way to learn why.
    let Some(read_only) = app.doc(id).map(|doc| doc.read_only) else {
        return Commit::Accepted;
    };

    if app.refuse_if_read_only(read_only) {
        return Commit::Refused;
    }
    let Some(doc) = app.doc(id) else {
        return Commit::Accepted;
    };
    // The no-store `save_cmd` captures `path` in its closure and would
    // republish at the OLD name after the rename landed — a save that
    // silently resurrects the old file.
    if doc.save_in_flight() {
        messages::warn(app, "can't rename while a save is in flight");
        return Commit::Refused;
    }

    let typed = app.title.text().to_string();
    if !title::is_valid_name(&typed) {
        messages::error(app, "that name can't be used for a file");
        return Commit::Refused;
    }

    // A pathless draft is a CREATE, not a rename: `materialize`'s bind-new
    // route already handles the collision correctly, and offering
    // `[R]eplace` there would overwrite a foreign file with a buffer we
    // have never observed — forbidden, since no CAS baseline exists to
    // check against.
    let Some(from) = doc.file_path.clone() else {
        crate::rename_create::bind_new(app, id, &typed, effects);
        return Commit::Accepted;
    };

    let to = target_path(&from, &typed);
    if to == from {
        return Commit::Accepted;
    }

    // Never published: this is a CREATE at the new name, not a rename of a
    // source that isn't there. `target_path` (not `rename_create::bind_new`,
    // which joins onto the workspace root) keeps the document in its own
    // directory.
    if app.db.is_some()
        && app
            .doc(id)
            .and_then(|d| d.doc_db())
            .is_some_and(|d| d.bind_new)
    {
        crate::save::bind_new_now(app, id, to);
        return Commit::Accepted;
    }

    // A `None` ticket means the store already reported the failure through
    // `on_store_failure` — a `Refused` here would let a doubled blur report
    // the same failure twice for no benefit; nothing about the TITLE itself
    // was wrong, so this still returns `Accepted`.
    if let Some(ticket) = crate::rename_create::enqueue_rename(app, id, &from, &to, effects) {
        app.rename = RenameState::Committing {
            doc: id,
            from,
            to,
            ticket,
        };
    }
    Commit::Accepted
}

/// `<parent-of-from>/<name>` — the rename stays in the file's own directory
/// (the title field rejects `/`, so `name` can never escape it), and `name`
/// is used VERBATIM (decision 10): the title now edits the whole file name,
/// extension included, so there is nothing left for this function to
/// re-append.
fn target_path(from: &Path, name: &str) -> PathBuf {
    let parent = from.parent().unwrap_or_else(|| Path::new(""));
    parent.join(name)
}

/// `Msg::RenameDone` — the no-store route's reply.
pub fn handle_rename_done(
    app: &mut App,
    generation: u32,
    result: Result<RenameOutcome, String>,
    effects: &mut Effects,
) {
    if app.rename.ticket() != Some(Ticket::Cmd(generation)) {
        return; // stale: dismissed and restarted since. Dropped silently.
    }
    apply_outcome(app, result, effects);
}

/// A `rune-db` rename ack, routed from `app::handle_db_event` (mirroring
/// `materialize_ack::handle_materialize_ack`).
pub fn handle_rename_ack(app: &mut App, op_id: u64, outcome: RenameOutcome, effects: &mut Effects) {
    if app.rename.ticket() != Some(Ticket::Db(op_id)) {
        return; // stale ticket, dropped silently
    }
    apply_outcome(app, Ok(outcome), effects);
}

/// A `rune-db` op died (`DbEvent::Err`) — resolve the rename machine if
/// `op_id` was the ticket it was waiting on. Without this, `RenameState`
/// only ever advances on `Ok`, so a died op leaves `Committing`/`Capturing`
/// wedged forever: `begin` refuses every later rename ("already in
/// progress") for the rest of the process, and after a died `[R]eplace`
/// the user never learns whether their destructive replace happened.
pub fn fail_op(app: &mut App, op_id: u64, error: String, effects: &mut Effects) {
    if app.rename.ticket() == Some(Ticket::Db(op_id)) {
        apply_outcome(app, Err(error), effects);
    }
}

/// The whole writer thread died (`DbEvent::Fatal`) — every `Db` ticket the
/// rename machine could be holding is dead with it, regardless of op id.
pub fn fail_all(app: &mut App, error: String, effects: &mut Effects) {
    if matches!(app.rename.ticket(), Some(Ticket::Db(_))) {
        apply_outcome(app, Err(error), effects);
    }
}

/// The one place every reply — both routes, every variant — resolves.
fn apply_outcome(app: &mut App, result: Result<RenameOutcome, String>, effects: &mut Effects) {
    let Some(doc_id) = app.rename.doc() else {
        return;
    };
    let (from, to) = match &app.rename {
        RenameState::Committing { from, to, .. } | RenameState::Capturing { from, to, .. } => {
            (from.clone(), to.clone())
        }
        _ => return,
    };
    let was_capturing = matches!(app.rename, RenameState::Capturing { .. });

    match result {
        Ok(RenameOutcome::Renamed { to, durable }) => {
            let created = from.as_os_str().is_empty();
            app.rename = RenameState::Idle;
            bind_to(app, doc_id, &to, effects);
            let verb = if created { "created" } else { "renamed to" };
            messages::info(app, format!("{verb} {}", display_name(&to)));
            if !durable {
                messages::warn(app, crate::materialize_ack::DURABILITY_UNCONFIRMED_WARNING);
            }
        }
        Ok(RenameOutcome::Replaced { displaced }) => {
            app.rename = RenameState::Idle;
            bind_to(app, doc_id, &to, effects);
            let name = display_name(&to);
            let text = match displaced.size {
                Some(size) => format!(
                    "replaced {name} \u{2014} its {size} bytes were preserved in the recovery store"
                ),
                None => {
                    format!(
                        "replaced {name} \u{2014} its bytes were preserved in the recovery store"
                    )
                }
            };
            messages::info(app, text);
        }
        Ok(RenameOutcome::Collided { seen }) => {
            // An empty `from` means this was a draft CREATE, not a rename:
            // refuse in the footer, never offer `[R]eplace` (the buffer has
            // no CAS baseline against a file we never observed).
            if from.as_os_str().is_empty() {
                draft_collision_refusal(app, &to);
                return_to_title(app);
                return;
            }
            // Enter `Collision` ONLY if the prompt is really on screen: a
            // Guard already up outranks a new one (`guard::set_guard` never
            // displaces), and waiting on an invisible prompt would leave the
            // title focused with every key going somewhere the user does
            // not expect (hazard 1).
            let raised = guard::set_guard_or_warn(
                app,
                GuardPrompt {
                    doc: doc_id,
                    kind: GuardKind::RenameCollision {
                        target: display_name(&to),
                    },
                },
                &format!(
                    "rename to {} refused \u{2014} a prompt is already showing",
                    display_name(&to)
                ),
            );
            if raised == guard::GuardRaise::Raised {
                app.rename = RenameState::Collision {
                    doc: doc_id,
                    from,
                    to,
                    seen,
                };
            } else {
                app.rename = RenameState::Idle;
            }
        }
        Ok(RenameOutcome::Refused { .. }) => {
            app.rename = RenameState::Idle;
            messages::error(
                app,
                format!(
                    "{} changed since you confirmed \u{2014} nothing was replaced",
                    display_name(&to)
                ),
            );
        }
        Err(e) => {
            app.rename = RenameState::Idle;
            let what = if was_capturing { "replace" } else { "rename" };
            messages::error(
                app,
                format!("could not {what} {}: {e}", display_name(&from)),
            );
            return_to_title(app);
        }
    }
}

/// Binds `doc_id` to its new path. Dirty state and `saved_version` are
/// deliberately **unchanged**: only two acts touch the destination (⌘S,
/// save-on-close) and a rename is neither, so a dirty document renames
/// and stays dirty. History keys off inode+device, which `renamex_np`
/// preserves, so nothing is orphaned.
///
/// Writes no focus of its own: this ack lands strictly after the blur that
/// fired the rename already moved focus off the title (`App::set_focus`'s
/// `next` is always the Editor on every path that can reach `begin`, and
/// this reply arrives later than that synchronous transition), so there is
/// nothing left for this function to move.
fn bind_to(app: &mut App, doc_id: DocumentId, to: &Path, effects: &mut Effects) {
    if let Some(doc) = app.doc_mut(doc_id) {
        doc.bind_path(to.to_path_buf());
    }
    if app.active == doc_id {
        let name = app.doc(doc_id).map(title::name_for).unwrap_or_default();
        app.title.seed(&name);
    }
    crate::explorer::refresh_for(app, to, effects);
}

/// A failed rename (or a dismissed collision prompt) returns the user to the
/// title field holding what they TYPED — never the old committed name.
/// Per gotcha 1: the field already holds the typed name on every failure
/// path (the only production reseeders are `switch_to`, `focus_title`,
/// `bind_to` and `handle_materialize_ack`, and the last two run on success
/// only), so this needs a focus change and nothing else — reseeding here
/// would resurrect the old name and discard the undo history the user just
/// built.
fn return_to_title(app: &mut App) {
    app.refocus_title();
}

/// `[R]eplace` was pressed and allowed. Clears the prompt, mints a fresh
/// ticket, and enqueues the one non-cancellable capture-then-swap op.
///
/// Writes no focus of its own — same reasoning as `bind_to`: by the time the
/// Collision Guard is even reachable, the blur that fired the original
/// commit already moved focus to the Editor, so there is nothing left here
/// to move.
pub fn replace_confirmed(app: &mut App) {
    let RenameState::Collision {
        doc,
        from,
        to,
        seen,
    } = app.rename.clone()
    else {
        return;
    };

    // Move to `Capturing` BEFORE clearing the modal: `clear_modal` calls
    // `on_prompt_dismissed`, which cancels a `Collision` — leaving the
    // order the other way round would immediately undo this confirmation.
    let Some(db_id) = app.doc(doc).and_then(|d| d.doc_db()).map(|d| d.db_id) else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };

    match db
        .store
        .rename_replace(rune_db::DocId(db_id), &from, &to, seen)
    {
        Ok(op_id) => {
            app.db_ops.insert(op_id, crate::db::PendingOp::new(doc));
            app.rename = RenameState::Capturing {
                doc,
                from,
                to,
                seen,
                ticket: Ticket::Db(op_id),
            };
            guard::clear_guard(app);
        }
        Err(e) => {
            app.rename = RenameState::Idle;
            guard::clear_guard(app);
            crate::materialize_ack::on_store_failure(app, e.to_string());
        }
    }
}

/// Called by `guard::clear_guard` whenever a `RenameCollision` Guard is
/// removed — by `Esc`, by an `Error` displacing it, by anything at all.
/// Holds up the second half of the global invariant: no `Collision` state
/// ever outlives its prompt.
///
/// Returns the user to the title field with the name they typed still in
/// it, so a cancelled collision is one keystroke from a different name.
pub fn on_prompt_dismissed(app: &mut App) {
    if !matches!(app.rename, RenameState::Collision { .. }) {
        return;
    }
    app.rename = RenameState::Idle;
    return_to_title(app);
}

/// Clears the machine when `doc` is closed out from under it
/// (`workspace::close_now`). Written to leave an unrelated in-flight rename
/// alone: only a state belonging to `doc` is dropped.
pub fn forget_document(app: &mut App, doc: DocumentId) {
    if app.rename.doc() != Some(doc) {
        return;
    }
    let had_prompt = matches!(app.rename, RenameState::Collision { .. });
    app.rename = RenameState::Idle;
    if had_prompt {
        // `clear_modal` would re-enter `on_prompt_dismissed`, which is
        // already a no-op now that the state is `Idle` — clearing the slot
        // directly here would bypass the sole-writer rule, so route
        // through it anyway and let the no-op absorb the second call.
        guard::clear_guard(app);
    }
}

/// A draft-create collision: a footer refusal, never a Guard (see
/// [`crate::rename_create::bind_new`]). Reached from `apply_outcome`'s
/// `Collided` arm only when `from` is empty, which is how a create is
/// distinguished from a rename.
fn draft_collision_refusal(app: &mut App, target: &Path) {
    app.rename = RenameState::Idle;
    messages::error(app, format!("{} already exists", display_name(target)));
}
