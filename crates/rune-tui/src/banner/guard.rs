//! The Guard modal's own state and key handling (split out of `banner.rs`
//! for the §1.6 budget, plan WP2: the `DirtyQuit` addition pushed the
//! parent file over): `GuardPrompt`/`GuardKind`, the `[S]ave`/`[D]iscard`/
//! `[Esc]` option labels every kind shares, and `handle_guard_key`'s
//! dispatch to each kind's own answer. `banner.rs` re-exports every public
//! item here, so `crate::banner::GuardKind` etc. keep resolving unchanged
//! for every existing caller.

use std::collections::BTreeMap;

use crate::app::{App, QuitIntent, StatusSource};
use crate::document::DocumentId;
use crate::keymap::{KeyCode, KeyInput};
use crate::runtime::Effects;
use crate::save::{self, SaveStart};

use super::Modal;

/// The close/quit-confirmation prompt for a dirty document (plan WP5.S3,
/// widened by WP2 for quit): armed by `workspace::request_close`/
/// `pane::handle_quit_key` when the document at `doc` is dirty (and, for
/// quit, unpreserved), and resolved by `handle_guard_key` below —
/// `[S]ave`/`[D]iscard`/`Esc`. `kind` distinguishes what the ANSWER should
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
    /// trustworthy recovery journal (`App::is_preserved`) — plan WP2, the
    /// fix for the "guard is impossible to exit from" defect. Distinct from
    /// `DirtyClose` so the SAME `s`/`d` keys can be answered with the
    /// correct continuation: `DirtyClose`'s `[D]iscard` only ever closes one
    /// document, but here it must complete the quit the user actually
    /// asked for.
    DirtyQuit,
    /// A rename whose destination already exists (§1.4.4's destructive
    /// transition). `[R]eplace` preserves the replaced file's bytes as a
    /// durable blob before destroying it (§1.4.10) — see `rename.rs`.
    /// `target` is the destination's display name, so the prompt can say
    /// WHICH file is about to be replaced rather than asking blind.
    RenameCollision {
        target: String,
    },
}

/// One `[X]abel` option in a Guard's footer chord list: `key` is the exact
/// char `handle_guard_key`'s answer arms match via `eq_ignore_ascii_case`;
/// `label` is what `footer.rs`'s `Mode::Guard` rendering shows for it. The
/// ONE source both sides read from (review fix: `footer.rs` previously
/// carried its own independently hand-maintained `[S]ave [D]iscard [Esc]
/// Cancel` literal, free to drift from this function's `s`/`d`/Esc match
/// arms).
pub struct GuardOption {
    pub key: char,
    pub label: &'static str,
}

pub const DIRTY_CLOSE_SAVE: GuardOption = GuardOption {
    key: 's',
    label: "[S]ave",
};
pub const DIRTY_CLOSE_DISCARD: GuardOption = GuardOption {
    key: 'd',
    label: "[D]iscard",
};
/// In display order — `footer.rs` iterates this for the Save/Discard pair;
/// `Esc`/Cancel isn't a `GuardOption` (it never triggers an ACTION beyond
/// clearing the modal, so there's no behavior to key off) and keeps its own
/// `DIRTY_CLOSE_CANCEL_LABEL` below instead. Shared verbatim by
/// `GuardKind::DirtyQuit` (plan WP2): the two prompts answer to the exact
/// same keys, only the CONTINUATION differs.
pub const DIRTY_CLOSE_OPTIONS: &[GuardOption] = &[DIRTY_CLOSE_SAVE, DIRTY_CLOSE_DISCARD];
pub const DIRTY_CLOSE_CANCEL_LABEL: &str = "[Esc] Cancel";

/// The rename-collision Guard's only action. `key` is the exact char
/// `handle_guard_key` matches; `label` is what the footer shows — the same
/// one-source-of-truth pairing `DIRTY_CLOSE_*` established.
pub const RENAME_REPLACE: GuardOption = GuardOption {
    key: 'r',
    label: "[R]eplace",
};
pub const RENAME_COLLISION_OPTIONS: &[GuardOption] = &[RENAME_REPLACE];

/// Names what Escape cancels for a given Guard kind. An exhaustive match, so
/// a future `GuardKind` variant is forced to choose its own cancellation
/// wording rather than silently inheriting a generic one.
pub(super) fn cancel_status(kind: &GuardKind) -> &'static str {
    match kind {
        GuardKind::DirtyClose => "close cancelled",
        GuardKind::DirtyQuit => "quit cancelled",
        GuardKind::RenameCollision { .. } => "rename cancelled",
    }
}

/// `Esc` cancels EVERY Guard kind identically — one arm, hoisted, so a
/// later kind can never forget to be cancellable — and reports what it
/// cancelled via `cancel_status` so the modal never just silently vanishes.
/// Every other key dispatches to the kind's own answer below.
pub(super) fn handle_guard_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    let Some(Modal::Guard(prompt)) = &app.modal else {
        return;
    };
    let doc = prompt.doc;
    if key.code == KeyCode::Escape {
        let msg = cancel_status(&prompt.kind);
        super::clear_modal(app);
        // A cancellation ack is the least important thing the status row can
        // say. An unacknowledged save failure is the most important, and the
        // footer already ranks it above ordinary status, so overwriting it
        // here would drop the user's only notice that their bytes did not
        // reach disk. Cancelling an unrelated Guard must never cost them
        // that; the save failure stays until its own success clears it.
        if app.status_source != StatusSource::SaveError {
            app.set_status(msg, StatusSource::Other);
        }
        return;
    }
    match &prompt.kind {
        GuardKind::DirtyClose => handle_dirty_close_key(app, doc, key, effects),
        GuardKind::DirtyQuit => handle_dirty_quit_key(app, key, effects),
        GuardKind::RenameCollision { .. } => handle_rename_collision_key(app, key),
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
/// no-op (plan WP5.S3).
fn handle_dirty_close_key(app: &mut App, doc: DocumentId, key: KeyInput, effects: &mut Effects) {
    match key.code {
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&DIRTY_CLOSE_SAVE.key) => {
            super::clear_modal(app);
            let _ = save::trigger_save(app, doc, effects);
            if app.doc(doc).is_some_and(|d| d.save_in_flight) {
                app.pending_close_on_save = Some(doc);
            }
        }
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&DIRTY_CLOSE_DISCARD.key) => {
            super::clear_modal(app);
            let _ = crate::workspace::close_now(app, doc, effects);
        }
        _ => {}
    }
}

/// The quit-guard's own answer (plan WP2, port of Go's `continuation{kind:
/// contQuit, ...}`): `d`/`D` discards immediately — Go parity, and the
/// prompt itself already said "Discard" — quitting right away rather than
/// routing through a save at all. `s`/`S` starts a save for EVERY dirty,
/// unpreserved document (not just the one the prompt named — quit is a
/// whole-session transition, so it must not leave a SECOND unpreserved
/// dirty document behind unresolved), building `App::quit_intent` from
/// whichever ones `trigger_save` actually started
/// (`SaveStart::InFlight`). Any refusal (`NeedsName`/`Refused`/already-
/// `InFlight`) leaves ITS OWN status up — `trigger_save` never returns a
/// refusal silently — and the whole quit intent is abandoned rather than
/// left waiting on a save that will never start: the modal still clears
/// (the user asked to answer, and got an answer: "here is why not"), but
/// nothing is wedged waiting on a save that will never complete.
fn handle_dirty_quit_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    match key.code {
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&DIRTY_CLOSE_DISCARD.key) => {
            super::clear_modal(app);
            app.should_quit = true;
        }
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&DIRTY_CLOSE_SAVE.key) => {
            super::clear_modal(app);
            start_quit_save_fan_out(app, effects);
        }
        _ => {}
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
/// document (plan WP1 decision 3) — a degraded store's FIRST `trigger_save`
/// for a given document only arms that slot rather than saving. Churning
/// through the rest of the fan-out after that would overwrite the slot with
/// a LATER document's arm, silently discarding this one's and leaving the
/// status naming whichever document happened to arm last rather than the
/// one the user is now staring at a sentence about. So the fan-out stops
/// dead the moment an iteration arms the slot for `id`: the surviving status
/// (`save.rs`'s degraded arm, which names `id`) stays true, and the obvious
/// next keystroke — ⌘S again — is answered by the confirm gate that is
/// actually armed. Any documents further down the list simply get their
/// turn on the next quit attempt, once this one is resolved.
fn start_quit_save_fan_out(app: &mut App, effects: &mut Effects) {
    let docs = crate::pane::unpreserved_dirty_docs(app);
    let mut pending = BTreeMap::new();
    for id in docs {
        match save::trigger_save(app, id, effects) {
            SaveStart::InFlight => {
                if let Some(version) = app.doc(id).and_then(|d| d.pending_save_version()) {
                    pending.insert(id, version);
                }
            }
            SaveStart::Refused if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) => {
                break;
            }
            SaveStart::NotDirty | SaveStart::NeedsName | SaveStart::Refused => {}
        }
    }
    app.quit_intent = if pending.is_empty() {
        None
    } else {
        Some(QuitIntent { pending })
    };
}

/// `r`/`R` confirms the destructive replace — but only when there is a
/// durable store to capture the displaced bytes into (§1.4.10). Without
/// one the key is a consumed no-op with an explanation, and the prompt
/// STAYS up: silently doing nothing would look like a dropped keypress,
/// and clearing it would look like the replace happened.
fn handle_rename_collision_key(app: &mut App, key: KeyInput) {
    let KeyCode::Char(c) = key.code else { return };
    if !c.eq_ignore_ascii_case(&RENAME_REPLACE.key) {
        return;
    }
    if !crate::rename::replace_allowed(app) {
        app.set_status(
            "cannot replace \u{2014} recovery store unavailable",
            StatusSource::Other,
        );
        return;
    }
    crate::rename::replace_confirmed(app);
}
