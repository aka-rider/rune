//! One-message step execution, split out of `driver` (§1.6 budget): builds
//! `(Msg, MsgTag)` pairs, discharges the deferred save/rename `Cmd`s, and
//! runs one `update` call under `catch_unwind`, checking every invariant
//! against the resulting `Snapshot`/`StepCtx`. Nothing here changes
//! behaviour beyond plan WP5's rename discharge — every other function is
//! exactly what `driver` used to define locally, now reached through
//! `step_exec::`.

use rune_tui::app::{self, App};
use rune_tui::keymap::{self, KeyInput};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::Vfs;

use crate::invariant::{self, Violation};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

use super::{Outcome, State, checks};

/// Builds the `(Msg, MsgTag)` pair for one keystroke — the one place
/// `keymap::resolve` is consulted for tagging, shared by `Action::Key` and
/// `Action::Type`'s per-char expansion.
pub(super) fn key_step(key: KeyInput) -> (Msg, MsgTag) {
    let tag = MsgTag::Key {
        input: key,
        command: keymap::resolve(key),
    };
    (Msg::Key(key), tag)
}

/// Runs the one deferred save `Cmd`, if any, returning the `Msg` it
/// produced together with its tag and the bytes it was constructed with —
/// looked up in the per-document snapshot by the ack's OWN `id`, never by
/// whichever document is active at delivery time (see `MsgTag::SaveDone`'s
/// docs; `TODO-fuzz-save-verbatim-help-doc-stale-ack.md`). The snapshot is
/// guaranteed to have an entry for `id`: it was built from every open
/// document at the moment THIS save `Cmd` was constructed, and `id` names
/// exactly the document `trigger_save` built the `Cmd` for — which must
/// have existed then (`trigger_save` bails out before constructing any
/// `Cmd` if its target document doesn't). `save_cmd` (`app.rs`) only ever
/// constructs a `Msg::SaveDone` reply and never returns `None` — the `None`
/// arms below are defensive against `Cmd`'s general contract, not reachable
/// from any real save `Cmd` this driver stores. Never synthesizes
/// `Msg::SaveDone` itself (G14) — it only ever forwards what `cmd.run()`
/// actually returned.
pub(super) fn discharge_pending_save(state: &mut State) -> Option<(Msg, MsgTag, Vec<u8>)> {
    let (cmd, per_doc_bytes) = state.pending_save.take()?;
    let msg = cmd.run()?;
    let Msg::SaveDone {
        id,
        version,
        result,
        ..
    } = &msg
    else {
        return None;
    };
    let tag = MsgTag::SaveDone {
        id: *id,
        version: *version,
        ok: result.is_ok(),
    };
    let bytes = per_doc_bytes.get(id).cloned().unwrap_or_default();
    Some((msg, tag, bytes))
}

/// Runs the one deferred no-store rename `Cmd`, if any, returning the
/// `Msg` it produced together with its tag (plan WP5). Without this, a
/// rename `Cmd` spawned by `rename::begin` and never delivered would leave
/// `app.rename` stuck in `RenameState::Committing` for the rest of the
/// session: `in_flight()` then refuses every later blur attempt,
/// including the end-of-session drive's own `^E` restore — which is
/// exactly the bug this closes (a rename `Cmd` used to be silently
/// dropped on the floor; only `CmdKind::Save` was ever tracked).
pub(super) fn discharge_pending_rename(state: &mut State) -> Option<(Msg, MsgTag)> {
    let cmd = state.pending_rename.take()?;
    let msg = cmd.run()?;
    if !matches!(msg, Msg::RenameDone { .. }) {
        return None;
    }
    Some((msg, MsgTag::RenameDone))
}

/// Delivers one message through `update`, captures the resulting `Snapshot`
/// and `StepCtx`, checks every invariant, and records a violation (if any)
/// into `outcome`. Returns `true` when the session must stop.
pub(super) fn step_and_check(
    state: &mut State,
    prev: &mut Snapshot,
    msg: Msg,
    tag: MsgTag,
    delivered_save_bytes: Option<Vec<u8>>,
    outcome: &mut Outcome,
) -> bool {
    state.steps += 1;
    let step_index = state.steps;
    let is_save_done_ok = matches!(&tag, MsgTag::SaveDone { ok: true, .. });

    let effects = match run_update_catching_panic(&mut state.app, msg) {
        Ok(effects) => effects,
        Err(payload) => {
            outcome.violation = Some(Violation {
                id: "NO-PANIC",
                message: downcast_panic(&payload),
            });
            outcome.final_snapshot = Some(prev.clone());
            outcome.final_ctx = None;
            return true;
        }
    };

    // Classify every Cmd this step produced (WP3.S7 rule 3): Save is
    // deferred (there can only ever be one, G9); QuitTimeout/ClipboardRead
    // are dropped — a headless driver must never sleep 2 real seconds or
    // fork `/usr/bin/pbpaste`. `state.pending_save` is an `Option`, never a
    // queue, PRECISELY because G9 claims at most one save `Cmd` is ever
    // outstanding — silently overwriting a still-pending one here would
    // drop the first save's `Cmd` on the floor (never delivered, `SAVE-
    // CLEAN-MATCHES-DISK` never even sampling it) and make this driver
    // blind to the exact G9 violation it exists to catch (CODE-REVIEW.md
    // rune-fuzz finding 3). A second in-flight save `Cmd` is therefore a
    // violation in its own right, not a silent overwrite.
    for cmd in effects.cmds {
        if cmd.kind() == CmdKind::Save {
            if state.pending_save.is_some() {
                outcome.violation = Some(Violation {
                    id: "SAVE-SINGLE-FLIGHT",
                    message: "a second save Cmd arrived while one was already pending \
                              (G9: at most one save Cmd may ever be outstanding)"
                        .to_string(),
                });
                outcome.final_snapshot = Some(prev.clone());
                outcome.final_ctx = None;
                return true;
            }
            // Snapshot EVERY open document's content now, at the instant
            // the `Cmd` is constructed — never just `prev.content` (the
            // ACTIVE document's `Snapshot`): `trigger_save` can be called
            // with an id other than `app.active` (a Guard modal's `s`
            // hotkey saves its own prompt's document), so the only
            // reliable way to recover "what bytes was this Cmd actually
            // built with" is to have all candidates on hand and pick the
            // right one once the ack names its `id`.
            let per_doc_bytes = state
                .app
                .documents
                .iter()
                .map(|(&id, doc)| (id, doc.buffer.content().as_bytes().to_vec()))
                .collect();
            state.pending_save = Some((cmd, per_doc_bytes));
        } else if cmd.kind() == CmdKind::Rename {
            // Structurally at most one at a time (`rename::begin` refuses a
            // second commit while `app.rename.in_flight()`), so overwriting
            // an existing `Some` here can never lose a still-outstanding
            // Cmd in practice — unlike `pending_save` above, there is no
            // separate per-document byte snapshot to carry: no rename-path
            // checker needs one yet.
            state.pending_rename = Some(cmd);
        }
    }

    if is_save_done_ok {
        state.saves_delivered_ok += 1;
    }

    // Plan WP5: `RenameDone` was just processed above (`run_update_
    // catching_panic` -> `rename::apply_outcome`), so `state.app`'s own
    // document model already names the SEED document's real current path
    // if the rename actually landed. `state.path` is separate driver
    // bookkeeping, documented (`SAVE-VERBATIM`/`SAVE-CLEAN-MATCHES-DISK`)
    // as "the real, seeded document's" location for the `ctx.disk` read
    // immediately below — it must be resynced HERE, before that read,
    // never after: `discharge_pending_rename` already ran the real
    // `rename_excl` against the real `Mem` before this message ever
    // reached `update`, so a resync any later would still compute `ctx.
    // disk` against the now-stale pre-rename path on this very step.
    if matches!(tag, MsgTag::RenameDone)
        && let Some(path) = state
            .app
            .doc(state.seed_doc)
            .and_then(|d| d.file_path.clone())
    {
        state.path = path;
    }

    let sampled = checks::should_sample(step_index);
    let next = Snapshot::capture(&mut state.app, sampled);
    let disk = state.mem.read(&state.path).ok();
    // `SAVE-CLEAN-MATCHES-DISK` only ever tests this field for `.is_some()`
    // (a save is still outstanding), never its content — any one entry
    // from the per-document snapshot signals that correctly.
    let pending_save_bytes = state
        .pending_save
        .as_ref()
        .and_then(|(_, per_doc)| per_doc.values().next().cloned());

    let ctx = StepCtx {
        step: step_index,
        msg: tag,
        raw: effects.raw,
        disk,
        pending_save_bytes,
        delivered_save_bytes,
        saves_delivered_ok: state.saves_delivered_ok,
    };

    let mut violation = invariant::check_all(prev, &next, &ctx);
    // `SYNC-IDEMPOTENT`/`WRAP-RT` need live `&mut App`/`ViewSnapshots`
    // access `Snapshot` can't carry (module docs) — checked only on
    // sampled steps (G19: the display pipeline dominates debug-build
    // runtime).
    if violation.is_none() && sampled {
        violation = checks::sync_idempotent_check(&mut state.app)
            .or_else(|| checks::wrap_rt_check(&state.app, next.line_count));
    }
    *prev = next;

    if let Some(v) = violation {
        // Capture cells unconditionally on the violating step even though
        // sampling is off for every WP3 checker (G19) — WP5's report needs
        // the rendered frame regardless of which invariant fired.
        outcome.final_snapshot = Some(Snapshot::capture(&mut state.app, true));
        outcome.final_ctx = Some(ctx);
        outcome.violation = Some(v);
        return true;
    }
    false
}

/// Wraps one `update` + `sync_view` call in `catch_unwind` — G13's three
/// live `debug_assert!`s (buffer line-starts, undo `reapply`, render cell-
/// map length) fire in this debug build, and any of them (or a genuine
/// panic anywhere in the pipeline) must produce a `NO-PANIC` violation
/// rather than aborting the whole fuzz run.
fn run_update_catching_panic(
    app: &mut App,
    msg: Msg,
) -> Result<Effects, Box<dyn std::any::Any + Send>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let mut effects = Effects::default();
        app::update(app, msg, &mut effects);
        app.sync_view();
        effects
    }))
}

/// The same downcast ladder proptest itself uses to render a caught panic's
/// payload.
fn downcast_panic(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<unknown panic value>".to_string()
    }
}
