//! The close/quit/rename/disk-conflict confirmation prompt — `App`'s one
//! `guard: Option<GuardPrompt>` slot (plan WP1: replaces the pre-WP1
//! `banner::Modal`'s `Guard` variant now that its `Error` sibling has moved
//! to the message log, `messages.rs`). `GuardPrompt`/`GuardKind`, the
//! `[S]ave`/`[D]iscard`/`[Esc]` option labels every kind shares, and
//! `handle_guard_key`'s dispatch to each kind's own answer.

use std::collections::BTreeMap;

use rune_db::ObsId;

use crate::app::{App, QuitIntent, StatusSource};
use crate::document::DocumentId;
use crate::keymap::{KeyCode, KeyInput};
use crate::merge::MergeIntent;
use crate::runtime::Effects;
use crate::save::{self, SaveStart};
use crate::workspace;

/// The one chokepoint that raises a new Guard prompt (plan WP1, replacing
/// the pre-WP1 `banner::set_modal`): never displaces a prompt already up —
/// with the old modal error banner gone, a Guard is the only thing that can ever be up,
/// so "never displace" now simply means "only while none is up". Returns
/// whether the prompt was actually raised, `#[must_use]` because a caller
/// that assumes it always was (`rename.rs`'s `Collision` entry in
/// particular) would otherwise wait forever on a prompt that never
/// appeared.
#[must_use]
pub fn set_guard(app: &mut App, prompt: GuardPrompt) -> bool {
    if app.guard.is_some() {
        return false;
    }
    app.guard = Some(prompt);
    true
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
    /// The save-time CAS conflict (plan WP6.S4): `doc`'s ⌘S found the disk
    /// bytes no longer match what it last read. `fresh_obs` is the
    /// observation `record_fresh_from_stat` recorded of the live disk bytes
    /// AT THE MOMENT the conflict was detected — `[S]ave anyway`'s retry
    /// baseline, so the retried CAS check is against fact, not the stale
    /// hash that just failed.
    DiskConflict {
        fresh_obs: ObsId,
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

/// The disk-conflict Guard's three answers (plan WP6.S4) — `handle_guard_
/// key`'s `s`/`d`/Esc dispatch above already covers Save/Discard/Cancel by
/// key, so only `[M]erge` needs a new key; `DISK_CONFLICT_SAVE`/`_DISCARD`
/// reuse the same `s`/`d` keys as `DIRTY_CLOSE_*` (this Guard never competes
/// with that one for the same modal slot) but carry their own labels — "Save
/// anyway" says what's actually happening, unlike a plain "Save" here.
pub const DISK_CONFLICT_SAVE: GuardOption = GuardOption {
    key: 's',
    label: "[S]ave anyway",
};
pub const DISK_CONFLICT_DISCARD: GuardOption = GuardOption {
    key: 'd',
    label: "[D]iscard",
};
pub const DISK_CONFLICT_MERGE: GuardOption = GuardOption {
    key: 'm',
    label: "[M]erge",
};
pub const DISK_CONFLICT_OPTIONS: &[GuardOption] = &[
    DISK_CONFLICT_SAVE,
    DISK_CONFLICT_DISCARD,
    DISK_CONFLICT_MERGE,
];

/// Names what Escape cancels for a given Guard kind. An exhaustive match, so
/// a future `GuardKind` variant is forced to choose its own cancellation
/// wording rather than silently inheriting a generic one.
fn cancel_status(kind: &GuardKind) -> &'static str {
    match kind {
        GuardKind::DirtyClose => "close cancelled",
        GuardKind::DirtyQuit => "quit cancelled",
        GuardKind::RenameCollision { .. } => "rename cancelled",
        GuardKind::DiskConflict { .. } => "save cancelled",
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
    if key.code == KeyCode::Escape {
        let msg = cancel_status(&prompt.kind);
        clear_guard(app);
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
        GuardKind::DiskConflict { fresh_obs } => {
            handle_disk_conflict_key(app, doc, *fresh_obs, key, effects);
        }
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
            clear_guard(app);
            let _ = save::trigger_save(app, doc, effects);
            if app.doc(doc).is_some_and(|d| d.save_in_flight) {
                app.pending_close_on_save = Some(doc);
            }
        }
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&DIRTY_CLOSE_DISCARD.key) => {
            clear_guard(app);
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
            clear_guard(app);
            app.should_quit = true;
        }
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&DIRTY_CLOSE_SAVE.key) => {
            clear_guard(app);
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

/// The disk-conflict Guard's own answer (plan WP6.S4). `s`/`S` retries the
/// SAME save with the CAS baseline advanced to `fresh_obs` — the observation
/// of what was actually on disk at conflict-detection time — so the retry's
/// expectation matches fact instead of the stale hash that just failed; a
/// disk change landing again in between just fails the retry into a fresh
/// conflict of its own, exactly like any other CAS race. `d`/`D` and `m`/`M`
/// both switch onto `doc` first (a conflict can in principle be answered for
/// a document that isn't the active one, e.g. a background quit-save) before
/// starting `merge::begin`'s shared entry pipeline, which reads its subject
/// off `app.active` (plan A2 — `Discard` shares `Merge`'s own fresh-`MergePrep`
/// round trip rather than repeating it). Every other key is a consumed
/// no-op, matching every other Guard kind.
fn handle_disk_conflict_key(
    app: &mut App,
    doc: DocumentId,
    fresh_obs: ObsId,
    key: KeyInput,
    effects: &mut Effects,
) {
    match key.code {
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&DISK_CONFLICT_SAVE.key) => {
            clear_guard(app);
            if let Some(doc_db) = app.doc_mut(doc).and_then(|d| d.db.as_mut()) {
                doc_db.expect_obs = fresh_obs;
            }
            let _ = save::trigger_save(app, doc, effects);
        }
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&DISK_CONFLICT_DISCARD.key) => {
            clear_guard(app);
            if app.active != doc {
                workspace::switch_to(app, doc);
            }
            crate::merge::begin(app, MergeIntent::Discard, effects);
        }
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&DISK_CONFLICT_MERGE.key) => {
            clear_guard(app);
            if app.active != doc {
                workspace::switch_to(app, doc);
            }
            crate::merge::begin(app, MergeIntent::Merge, effects);
        }
        _ => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        App::new(Buffer::new("hi"), None, Arc::new(Mem::new()), None)
    }

    fn prompt(doc: DocumentId, kind: GuardKind) -> GuardPrompt {
        GuardPrompt { doc, kind }
    }

    /// `set_guard` raises a prompt onto an empty slot.
    #[test]
    fn set_guard_raises_onto_an_empty_slot() {
        let mut app = app();
        let doc = app.active;
        assert!(set_guard(&mut app, prompt(doc, GuardKind::DirtyClose)));
        assert!(matches!(
            app.guard,
            Some(GuardPrompt {
                kind: GuardKind::DirtyClose,
                ..
            })
        ));
    }

    /// `set_guard` never displaces a prompt already up (plan WP1: with
    /// the old modal error banner gone, this is the whole of the pre-WP1 priority rule)
    /// — the `false` return is what lets `rename.rs` notice and stay `Idle`
    /// rather than wait on a prompt that was never raised.
    #[test]
    fn set_guard_refuses_to_replace_an_existing_prompt() {
        let mut app = app();
        let doc = app.active;
        assert!(set_guard(&mut app, prompt(doc, GuardKind::DirtyClose)));
        assert!(!set_guard(
            &mut app,
            prompt(
                doc,
                GuardKind::RenameCollision {
                    target: "b.md".to_string(),
                }
            )
        ));
        assert!(matches!(
            app.guard,
            Some(GuardPrompt {
                kind: GuardKind::DirtyClose,
                ..
            })
        ));
    }

    /// `clear_guard` empties the slot.
    #[test]
    fn clear_guard_empties_the_slot() {
        let mut app = app();
        let doc = app.active;
        assert!(set_guard(&mut app, prompt(doc, GuardKind::DirtyClose)));
        clear_guard(&mut app);
        assert!(app.guard.is_none());
    }

    /// A cleared `RenameCollision` prompt notifies the rename machine
    /// (`rename::on_prompt_dismissed`), returning it to `Idle` — the other
    /// half of the global invariant `rename.rs` documents.
    #[test]
    fn clear_guard_on_a_rename_collision_returns_the_rename_machine_to_idle() {
        let mut app = app();
        let doc = app.active;
        app.rename = crate::rename::RenameState::Collision {
            doc,
            from: std::path::PathBuf::new(),
            to: std::path::PathBuf::from("/b.md"),
            seen: rune_vfs::Stat {
                size: 0,
                mtime: std::time::SystemTime::UNIX_EPOCH,
                identity: rune_vfs::Identity::default(),
                nlink: None,
                kind: rune_vfs::FileKind::File,
            },
        };
        assert!(set_guard(
            &mut app,
            prompt(
                doc,
                GuardKind::RenameCollision {
                    target: "b.md".to_string(),
                }
            )
        ));
        clear_guard(&mut app);
        assert!(app.guard.is_none());
        assert_eq!(app.rename, crate::rename::RenameState::Idle);
    }
}
