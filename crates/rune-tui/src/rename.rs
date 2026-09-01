use std::path::{Path, PathBuf};

use rune_db::RenameOutcome;
use rune_vfs::Stat;

use crate::app::App;
use crate::document::DocumentId;
use crate::generation::RenameGen as Generation;
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::messages;
use crate::resolved::ResolvedPath;
use crate::runtime::{CmdError, Effects};
use crate::title;
use crate::workspace::resolve_or_report;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ticket {
    Cmd(Generation),
    Db(u64),
}

/// No `Replacing` state: capture-then-swap is one non-cancellable `rune-db`
/// op, so "swapped but not captured" is never representable. No `Failed`
/// state: every failure edge returns to `Idle` after logging.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum RenameState {
    #[default]
    Idle,
    /// A no-clobber rename is in flight.
    Committing {
        doc: DocumentId,
        from: PathBuf,
        to: ResolvedPath,
        ticket: Ticket,
        draft_baseline: Option<crate::document::SaveCapture>,
    },
    /// The destination exists and the user is being asked.
    Collision {
        doc: DocumentId,
        from: PathBuf,
        to: ResolvedPath,
        /// The destination's stat at the moment of collision — the
        /// **consent** baseline ("still the file you agreed to replace?"),
        /// not the safety mechanism; safety comes from `rune-db`'s own
        /// post-swap capture.
        seen: Stat,
    },
    /// The confirmed destructive replace is in flight.
    Capturing {
        doc: DocumentId,
        from: PathBuf,
        to: ResolvedPath,
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

    pub(crate) fn in_flight(&self) -> bool {
        self.ticket().is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    Accepted,
    Refused,
}

fn collision_doc(app: &App) -> Option<DocumentId> {
    match &app.rename {
        RenameState::Collision { doc, .. } => Some(*doc),
        _ => None,
    }
}

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

pub fn begin(app: &mut App, effects: &mut Effects) -> Commit {
    if app.rename.in_flight() {
        messages::warn_if_new(app, "a rename is already in progress");
        return Commit::Refused;
    }
    if app.trash.in_flight() {
        messages::warn_if_new(app, "can't rename while a trash is in progress");
        return Commit::Refused;
    }

    let id = app.active;
    let Some(read_only) = app.doc(id).map(|doc| doc.read_only) else {
        return Commit::Accepted;
    };

    if app.refuse_if_read_only(read_only) {
        return Commit::Refused;
    }
    let Some(doc) = app.doc(id) else {
        return Commit::Accepted;
    };
    // A save's `Cmd` closure captures the old path at trigger time; a
    // rename racing it would resurrect the file under that stale name.
    if doc.save_in_flight() {
        messages::warn_if_new(app, "can't rename while a save is in flight");
        return Commit::Refused;
    }

    let typed = app.title.text().to_string();
    if !title::is_valid_name(&typed) {
        messages::error_if_new(app, "that name can't be used for a file");
        return Commit::Refused;
    }

    let Some(from) = doc
        .resolved_path()
        .cloned()
        .map(ResolvedPath::into_path_buf)
    else {
        return crate::rename_create::bind_new(app, id, &typed, effects);
    };

    let Some(to) = resolve_or_report(app, &target_path(&from, &typed), "rename to") else {
        return Commit::Refused;
    };
    if to.as_path() == from {
        return Commit::Accepted;
    }

    if app.db.is_some()
        && app
            .doc(id)
            .and_then(|d| d.doc_db())
            .is_some_and(|d| d.publish_mode.is_create_only())
    {
        let Ok(clearance) =
            crate::save::gate::clear(app, id, crate::save::gate::SaveEntry::BindNew)
        else {
            return Commit::Refused;
        };
        crate::save::bind_new_now(app, id, to, &clearance);
        return Commit::Accepted;
    }

    if let Some(ticket) = crate::rename_create::enqueue_rename(app, id, &from, &to, effects) {
        app.rename = RenameState::Committing {
            doc: id,
            from,
            to,
            ticket,
            draft_baseline: None,
        };
    }
    Commit::Accepted
}

fn target_path(from: &Path, name: &str) -> PathBuf {
    let parent = from.parent().unwrap_or_else(|| Path::new(""));
    parent.join(name)
}

pub fn handle_rename_done(
    app: &mut App,
    generation: Generation,
    result: Result<RenameOutcome, CmdError>,
    effects: &mut Effects,
) {
    if app.rename.ticket() != Some(Ticket::Cmd(generation)) {
        return;
    }
    apply_outcome(app, result, effects);
}

pub fn handle_rename_ack(app: &mut App, op_id: u64, outcome: RenameOutcome, effects: &mut Effects) {
    if app.rename.ticket() != Some(Ticket::Db(op_id)) {
        return;
    }
    apply_outcome(app, Ok::<_, CmdError>(outcome), effects);
}

/// Without this, `RenameState` only ever advances on `Ok`, so a died op
/// leaves `Committing`/`Capturing` wedged forever — `begin` would refuse
/// every later rename for the rest of the process.
pub fn fail_op(app: &mut App, op_id: u64, error: String, effects: &mut Effects) {
    if app.rename.ticket() == Some(Ticket::Db(op_id)) {
        apply_outcome(app, Err(CmdError::Refused(error)), effects);
    }
}

/// Every `Db` ticket the rename machine could be holding dies with the
/// writer thread, regardless of op id.
pub fn fail_all(app: &mut App, error: String, effects: &mut Effects) {
    if matches!(app.rename.ticket(), Some(Ticket::Db(_))) {
        apply_outcome(app, Err(CmdError::Refused(error)), effects);
    }
}

fn apply_outcome(app: &mut App, result: Result<RenameOutcome, CmdError>, effects: &mut Effects) {
    let Some(doc_id) = app.rename.doc() else {
        return;
    };
    let (from, to, draft_baseline) = match &app.rename {
        RenameState::Committing {
            from,
            to,
            draft_baseline,
            ..
        } => (from.clone(), to.clone(), draft_baseline.clone()),
        RenameState::Capturing { from, to, .. } => (from.clone(), to.clone(), None),
        _ => return,
    };
    let was_capturing = matches!(app.rename, RenameState::Capturing { .. });

    match result {
        Ok(RenameOutcome::Renamed { durable }) => {
            let created = from.as_os_str().is_empty();
            app.rename = RenameState::Idle;
            bind_to(app, doc_id, to.clone(), effects);
            if created
                && let Some(capture) = draft_baseline
                && let Some(doc) = app.doc_mut(doc_id)
            {
                doc.adopt_saved(capture.version, capture.content);
            }
            let verb = if created { "created" } else { "renamed to" };
            messages::info(app, format!("{verb} {}", display_name(&to)));
            if !durable {
                messages::warn(app, crate::materialize_ack::DURABILITY_UNCONFIRMED_WARNING);
            }
        }
        Ok(RenameOutcome::Replaced { displaced, durable }) => {
            app.rename = RenameState::Idle;
            bind_to(app, doc_id, to.clone(), effects);
            let name = display_name(&to);
            let text = displaced.size.map_or_else(
                || format!("replaced {name} \u{2014} its bytes were preserved in the recovery store"),
                |size| {
                    format!(
                        "replaced {name} \u{2014} its {size} bytes were preserved in the recovery store"
                    )
                },
            );
            messages::info(app, text);
            if !durable {
                messages::warn(app, crate::materialize_ack::DURABILITY_UNCONFIRMED_WARNING);
            }
        }
        Ok(RenameOutcome::Collided { seen }) => {
            if from.as_os_str().is_empty() {
                draft_collision_refusal(app, &to);
                return_to_title(app, doc_id);
                return;
            }
            if app.doc(doc_id).is_none() {
                app.rename = RenameState::Idle;
                messages::error(
                    app,
                    format!(
                        "{} already exists \u{2014} nothing was renamed",
                        display_name(&to)
                    ),
                );
                return;
            }
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
                effects,
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
            return_to_title(app, doc_id);
        }
    }
}

fn bind_to(app: &mut App, doc_id: DocumentId, to: ResolvedPath, effects: &mut Effects) {
    app.rebind_document_path(doc_id, to.clone());
    if app.active == doc_id {
        let name = app.doc(doc_id).map(title::name_for).unwrap_or_default();
        app.title.seed(&name);
    }
    crate::explorer::refresh_for(app, to.as_path(), effects);
}

fn return_to_title(app: &mut App, doc_id: DocumentId) {
    if app.active == doc_id {
        app.refocus_title();
    }
}

pub fn forget_document(app: &mut App, doc: DocumentId) {
    if app.rename.doc() != Some(doc) {
        return;
    }
    if app.rename.in_flight() {
        return;
    }
    let had_prompt = matches!(app.rename, RenameState::Collision { .. });
    app.rename = RenameState::Idle;
    if had_prompt {
        // `on_prompt_dismissed` is already a no-op once the state is
        // `Idle`, so routing through `clear_guard` here (rather than
        // writing `app.guard = None` directly) keeps it the sole writer
        // and just lets the no-op absorb this second call.
        guard::clear_guard(app);
    }
}

fn draft_collision_refusal(app: &mut App, target: &Path) {
    app.rename = RenameState::Idle;
    messages::error(app, format!("{} already exists", display_name(target)));
}

#[path = "rename_collision.rs"]
mod collision;
pub use collision::{on_prompt_dismissed, replace_allowed, replace_confirmed};

#[cfg(test)]
#[path = "rename_tests.rs"]
mod tests;
