//! The close/quit/rename/disk-conflict confirmation prompt — `App`'s one
//! `guard: Option<GuardPrompt>` slot, replacing the old `banner::Modal`'s
//! `Guard` variant now that its `Error` sibling has moved to the message
//! log, `messages.rs`. `GuardPrompt`/`GuardKind`, the save/discard/cancel
//! options every kind shares, and `handle_guard_key`'s dispatch to each
//! kind's own answer.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::app::{App, QuitIntent};
use crate::document::DocumentId;
use crate::keymap::{KeyCode, KeyInput};
use crate::merge::{MergeIntent, MergeState};
use crate::messages;
use crate::runtime::Effects;
use crate::save::{self, SaveMode, SaveStart};
use crate::workspace;

/// Whether [`set_guard`] actually raised its prompt. `#[must_use]` because a
/// caller that assumes `Raised` always was (`rename.rs`'s `Collision` entry
/// in particular) would otherwise wait forever on a prompt that never
/// appeared, and a `Displaced` a caller drops without telling the user
/// leaves whatever it was arming (a Trash/DirtyQuit confirmation) silently
/// gone.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardRaise {
    Raised,
    Displaced,
}

/// The one chokepoint that raises a new Guard prompt, replacing the old
/// `banner::set_modal`: never displaces a prompt already up — with the old
/// modal error banner gone, a Guard is the only thing that can ever be up,
/// so "never displace" now simply means "only while none is up".
pub fn set_guard(app: &mut App, prompt: GuardPrompt) -> GuardRaise {
    if app.guard.is_some() {
        return GuardRaise::Displaced;
    }
    app.guard = Some(prompt);
    GuardRaise::Raised
}

/// [`set_guard`] plus its callers' shared reaction to `Displaced`: post
/// `refused` to the message log and return the raise so a caller that
/// needs to know which one happened (`rename.rs`'s `Collision` entry) can
/// still branch on it, without also warning a second time itself.
pub fn set_guard_or_warn(app: &mut App, prompt: GuardPrompt, refused: &str) -> GuardRaise {
    let raise = set_guard(app, prompt);
    if raise == GuardRaise::Displaced {
        messages::warn(app, refused);
    }
    raise
}

/// The SOLE writer of `app.guard = None` — every dismissal path routes
/// here, including `set_guard`'s own indirect callers once the previous
/// state must go.
pub fn clear_guard(app: &mut App) {
    let was_collision = matches!(
        &app.guard,
        Some(GuardPrompt {
            kind: GuardKind::RenameCollision { .. },
            ..
        })
    );
    app.guard = None;
    if was_collision {
        crate::rename::on_prompt_dismissed(app);
    }
}

/// Self-retraction for the `DiskConflict` Guard: a later confirmed probe
/// for `doc` finding disk no longer diverged from this session's own
/// reconstruction means the CAS mismatch that raised the prompt is gone —
/// clearing it here beats leaving the user staring at a conflict that no
/// longer exists. A no-op for any other Guard kind or document.
pub(crate) fn retract_disk_conflict_on_convergence(
    app: &mut App,
    doc: DocumentId,
    kind: rune_db::SyncKind,
) {
    if kind.is_disk_divergent() {
        return;
    }
    let raised_for_doc = matches!(
        &app.guard,
        Some(GuardPrompt {
            doc: d,
            kind: GuardKind::DiskConflict,
        }) if *d == doc
    );
    if !raised_for_doc {
        return;
    }
    clear_guard(app);
    messages::info(app, "disk settled — save again when ready");
}

/// The close/quit-confirmation prompt for a dirty document: armed by
/// `workspace::request_close`/
/// `pane::handle_quit_key` when the document at `doc` is dirty (and, for
/// quit, unpreserved), and resolved by `handle_guard_key` below —
/// `s`/`d`/`Esc`. `kind` distinguishes what the ANSWER should
/// actually do (close vs. quit) — the prompt text and options otherwise
/// look the same to the user.
pub struct GuardPrompt {
    pub doc: DocumentId,
    pub kind: GuardKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardKind {
    DirtyClose,
    /// Quit is waiting on `doc` specifically because it has no live,
    /// trustworthy recovery journal (`App::is_preserved`) — otherwise quit
    /// would be impossible to exit from once armed. Distinct from
    /// `DirtyClose` so the SAME `s`/`d` keys can be answered with the
    /// correct continuation: `DirtyClose`'s discard only ever closes one
    /// document, but here it must complete the quit the user actually
    /// asked for.
    DirtyQuit,
    /// A rename whose destination already exists — a destructive
    /// transition. Replace preserves the replaced file's bytes as a
    /// durable blob before destroying it — see `rename.rs`.
    /// `target` is the destination's display name, so the prompt can say
    /// WHICH file is about to be replaced rather than asking blind.
    RenameCollision {
        target: String,
    },
    /// The save-time CAS conflict: `doc`'s ^S found the disk bytes no
    /// longer match what it last read. Save-anyway from here is a
    /// force-save, not a CAS retry — it bypasses the comparison entirely
    /// rather than re-running it against a fresher baseline, so a disk that
    /// keeps moving can never make the escape hatch itself refuse.
    DiskConflict,
    /// A user-requested move of `path` to the OS Trash — an Explorer
    /// selection or the active document's file. `path` is authoritative:
    /// an Explorer selection need not be the open document, so
    /// `GuardPrompt.doc` (set to `app.active`) is unused by this kind.
    Trash {
        path: PathBuf,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GuardKey {
    Char(char),
    Escape,
}

impl GuardKey {
    pub fn label(self) -> String {
        match self {
            GuardKey::Char(c) => c.to_ascii_uppercase().to_string(),
            GuardKey::Escape => "Esc".to_string(),
        }
    }
}

/// One option in a Guard's footer chord list: the key that answers it and
/// the help text naming what answering does. The ONE source both the
/// footer's rendering and `handle_guard_key`'s dispatch read from.
pub struct GuardOption {
    pub key: GuardKey,
    pub help: &'static str,
}

impl GuardOption {
    fn answers(&self, key: KeyInput) -> bool {
        match (self.key, key.code) {
            (GuardKey::Char(want), KeyCode::Char(got)) => got.eq_ignore_ascii_case(&want),
            (GuardKey::Escape, KeyCode::Escape) => true,
            _ => false,
        }
    }
}

pub const DIRTY_CLOSE_SAVE: GuardOption = GuardOption {
    key: GuardKey::Char('s'),
    help: "save",
};
pub const DIRTY_CLOSE_DISCARD: GuardOption = GuardOption {
    key: GuardKey::Char('d'),
    help: "discard",
};
/// In display order. Shared verbatim by `GuardKind::DirtyQuit`: the two
/// prompts answer to the exact same keys, only the CONTINUATION differs.
pub const DIRTY_CLOSE_OPTIONS: &[GuardOption] = &[DIRTY_CLOSE_SAVE, DIRTY_CLOSE_DISCARD];

/// Every Guard kind cancels the same way, so the footer appends this after
/// whatever options the kind itself offers — including the rename collision
/// whose only action can be withheld, leaving cancel the sole answer.
pub const GUARD_CANCEL: GuardOption = GuardOption {
    key: GuardKey::Escape,
    help: "cancel",
};

pub const RENAME_REPLACE: GuardOption = GuardOption {
    key: GuardKey::Char('r'),
    help: "replace",
};
pub const RENAME_COLLISION_OPTIONS: &[GuardOption] = &[RENAME_REPLACE];

/// The disk-conflict Guard's three answers. Save and Discard reuse the same
/// `s`/`d` keys as `DIRTY_CLOSE_*` — this Guard never competes with that one
/// for the same modal slot — but "save anyway" says what is actually
/// happening here, unlike a plain "save".
pub const DISK_CONFLICT_SAVE: GuardOption = GuardOption {
    key: GuardKey::Char('s'),
    help: "save anyway",
};
pub const DISK_CONFLICT_DISCARD: GuardOption = GuardOption {
    key: GuardKey::Char('d'),
    help: "discard",
};
pub const DISK_CONFLICT_MERGE: GuardOption = GuardOption {
    key: GuardKey::Char('m'),
    help: "merge",
};
pub const DISK_CONFLICT_OPTIONS: &[GuardOption] = &[
    DISK_CONFLICT_SAVE,
    DISK_CONFLICT_DISCARD,
    DISK_CONFLICT_MERGE,
];

/// The trash Guard's only action.
pub const TRASH_YES: GuardOption = GuardOption {
    key: GuardKey::Char('y'),
    help: "yes",
};
pub const TRASH_OPTIONS: &[GuardOption] = &[TRASH_YES];

/// Names what Escape cancels for a given Guard kind. An exhaustive match, so
/// a future `GuardKind` variant is forced to choose its own cancellation
/// wording rather than silently inheriting a generic one.
fn cancel_status(kind: &GuardKind) -> &'static str {
    match kind {
        GuardKind::DirtyClose => "close cancelled",
        GuardKind::DirtyQuit => "quit cancelled",
        GuardKind::RenameCollision { .. } => "rename cancelled",
        GuardKind::DiskConflict => "save cancelled",
        GuardKind::Trash { .. } => "trash cancelled",
    }
}

/// `Esc` cancels EVERY Guard kind identically — one arm, hoisted, so a
/// later kind can never forget to be cancellable — and reports what it
/// cancelled via `cancel_status` so the modal never just silently vanishes.
/// Every other key dispatches to the kind's own answer below.
pub(crate) fn handle_guard_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    let Some(prompt) = &app.guard else {
        return;
    };
    let doc = prompt.doc;
    if GUARD_CANCEL.answers(key) {
        let msg = cancel_status(&prompt.kind);
        clear_guard(app);
        // The log never clears an earlier entry, so an unacknowledged save
        // failure stays visible in the pane regardless of this cancellation
        // ack landing after it. Dismissing the disk-conflict Guard this way
        // needs no special case either: `footer::mode` ranks `Guard` above
        // `DiskChanged` only while `app.guard` is `Some`, so clearing it
        // here already lets the footer fall through to the persistent
        // disk-changed merge hint on its own.
        messages::info(app, msg);
        return;
    }
    match &prompt.kind {
        GuardKind::DirtyClose => handle_dirty_close_key(app, doc, key, effects),
        GuardKind::DirtyQuit => handle_dirty_quit_key(app, key, effects),
        GuardKind::RenameCollision { .. } => handle_rename_collision_key(app, key),
        GuardKind::DiskConflict => {
            handle_disk_conflict_key(app, doc, key, effects);
        }
        GuardKind::Trash { path } => handle_trash_key(app, path.clone(), key, effects),
    }
}

/// `s`/`S` saves `prompt.doc` then closes it — but ONLY once `trigger_save`
/// actually started a save (`doc.save_in_flight` true right after calling
/// it): a document with no file path, or one that just armed the degraded-
/// store confirm gate instead of saving, never gets its `save_in_flight`
/// set, so `pending_close_on_save` is deliberately left `None` in that
/// case — the close intent is dropped (the user must press `^w` again once
/// ready), never silently mis-fired against a save that never happened.
/// `d`/`D` discards and closes immediately. Every other key is a consumed
/// no-op.
fn handle_dirty_close_key(app: &mut App, doc: DocumentId, key: KeyInput, effects: &mut Effects) {
    if DIRTY_CLOSE_SAVE.answers(key) {
        clear_guard(app);
        let _ = save::trigger_save(app, doc, SaveMode::Normal, effects);
        if app
            .doc(doc)
            .is_some_and(super::document::Document::save_in_flight)
        {
            app.pending_close_on_save = Some(doc);
        }
    } else if DIRTY_CLOSE_DISCARD.answers(key) {
        clear_guard(app);
        let _ = crate::workspace::close_now(app, doc, effects);
    }
}

/// The quit-guard's own answer: `d`/`D` discards immediately,
/// and the prompt itself already said "Discard" — quitting right away
/// rather than routing through a save at all. `s`/`S` starts a save for EVERY dirty,
/// unpreserved document (not just the one the prompt named — quit is a
/// whole-session transition, so it must not leave a SECOND unpreserved
/// dirty document behind unresolved), building `App::quit`'s `SaveFanOut` from
/// whichever ones `trigger_save` actually started
/// (`SaveStart::InFlight`). Any refusal (`NeedsName`/`Refused`/already-
/// `InFlight`) leaves ITS OWN status up — `trigger_save` never returns a
/// refusal silently — and the whole quit intent is abandoned rather than
/// left waiting on a save that will never start: the modal still clears
/// (the user asked to answer, and got an answer: "here is why not"), but
/// nothing is wedged waiting on a save that will never complete.
fn handle_dirty_quit_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    if DIRTY_CLOSE_DISCARD.answers(key) {
        clear_guard(app);
        app.should_quit = true;
    } else if DIRTY_CLOSE_SAVE.answers(key) {
        clear_guard(app);
        start_quit_save_fan_out(app, effects);
    }
}

/// Calls `trigger_save` for every dirty, unpreserved document, correlating
/// the ones that actually started (`SaveStart::InFlight`) into a fresh
/// `QuitIntent`. A refusal (already covered by its own status message) is
/// simply not counted — if NONE started, there is nothing left for quit to
/// wait on, so no `QuitIntent` is armed at all and the user's status line
/// already says why (`trigger_save`'s own refusal arms all set one).
///
/// `App::pending_save_confirm` is a single global slot, not one per
/// document — a degraded store's FIRST `trigger_save`
/// for a given document only arms that slot rather than saving. Churning
/// through the rest of the fan-out after that would overwrite the slot with
/// a LATER document's arm, silently discarding this one's and leaving the
/// status naming whichever document happened to arm last rather than the
/// one the user is now staring at a sentence about. So the fan-out stops
/// dead the moment an iteration arms the slot for `id`: the surviving status
/// (`save.rs`'s degraded arm, which names `id`) stays true, and the obvious
/// next keystroke — ^S again — is answered by the confirm gate that is
/// actually armed. Any documents further down the list simply get their
/// turn on the next quit attempt, once this one is resolved.
fn start_quit_save_fan_out(app: &mut App, effects: &mut Effects) {
    let docs = crate::pane::unpreserved_dirty_docs(app);
    let mut pending = BTreeMap::new();
    for id in docs {
        match save::trigger_save(app, id, SaveMode::Normal, effects) {
            SaveStart::InFlight => {
                if let Some(version) = app
                    .doc(id)
                    .and_then(super::document::Document::pending_save_version)
                {
                    pending.insert(id, version);
                }
            }
            SaveStart::Refused if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) => {
                break;
            }
            SaveStart::NotDirty | SaveStart::NeedsName | SaveStart::Refused => {}
        }
    }
    app.quit = if pending.is_empty() {
        crate::app::QuitNegotiation::Idle
    } else {
        crate::app::QuitNegotiation::SaveFanOut(QuitIntent { pending })
    };
}

/// `r`/`R` confirms the destructive replace — but only when there is a
/// durable store to capture the displaced bytes into first. Without
/// one the key is a consumed no-op with an explanation, and the prompt
/// STAYS up: silently doing nothing would look like a dropped keypress,
/// and clearing it would look like the replace happened.
fn handle_rename_collision_key(app: &mut App, key: KeyInput) {
    if !RENAME_REPLACE.answers(key) {
        return;
    }
    if !crate::rename::replace_allowed(app) {
        messages::error(app, "cannot replace \u{2014} recovery store unavailable");
        return;
    }
    crate::rename::replace_confirmed(app);
}

/// The disk-conflict Guard's own answer. `s`/`S` is a FORCE-save — it
/// bypasses the compare-and-swap entirely (`SaveMode::Force`) rather than
/// retrying it against a fresher baseline: existence-aware publish and
/// unconditional displaced-byte capture make it structurally un-refusable, so
/// a disk that keeps moving between the conflict and this keypress can never
/// make the escape hatch itself refuse again. `d`/`D` and `m`/`M`
/// both switch onto `doc` first (a conflict can in principle be answered for
/// a document that isn't the active one, e.g. a background quit-save) before
/// starting `merge::begin`'s shared entry pipeline, which reads its subject
/// off `app.active` — `Discard` shares `Merge`'s own fresh-`MergePrep`
/// round trip rather than repeating it. Every other key is a consumed
/// no-op, matching every other Guard kind.
fn handle_disk_conflict_key(app: &mut App, doc: DocumentId, key: KeyInput, effects: &mut Effects) {
    if DISK_CONFLICT_SAVE.answers(key) {
        // The consent this prompt exists to capture must survive a refused
        // attempt: `trigger_save` runs FIRST, and the prompt is torn down
        // only once THIS press actually started a save — a save already in
        // flight before this press (its own warning already posted by
        // `trigger_save`), a rename in flight, or unresolved merge conflicts
        // each leave the Guard up so the user's "save anyway" is never
        // silently dropped on the floor, and a repeat press once the
        // in-flight save has finished can still answer it.
        let already_in_flight = app
            .doc(doc)
            .is_some_and(super::document::Document::save_in_flight);
        let start = save::trigger_save(app, doc, SaveMode::Force, effects);
        if !already_in_flight && matches!(start, SaveStart::InFlight) {
            clear_guard(app);
        }
    } else if DISK_CONFLICT_DISCARD.answers(key) {
        if app.active != doc {
            workspace::switch_to(app, doc);
        }
        crate::merge::begin(app, MergeIntent::Discard, effects);
        clear_guard_if_begin_started(app, doc);
    } else if DISK_CONFLICT_MERGE.answers(key) {
        if app.active != doc {
            workspace::switch_to(app, doc);
        }
        crate::merge::begin(app, MergeIntent::Merge, effects);
        clear_guard_if_begin_started(app, doc);
    }
}

fn clear_guard_if_begin_started(app: &mut App, doc: DocumentId) {
    if matches!(&app.merge, MergeState::Pending { doc: d, .. } if *d == doc) {
        clear_guard(app);
    }
}

/// `y`/`Y` confirms the trash. Every other key is a consumed no-op,
/// matching every other Guard kind.
fn handle_trash_key(app: &mut App, path: PathBuf, key: KeyInput, effects: &mut Effects) {
    if !TRASH_YES.answers(key) {
        return;
    }
    clear_guard(app);
    crate::trash::confirm(app, path, effects);
}

#[cfg(test)]
#[path = "guard_tests.rs"]
mod tests;
