//! One-message step execution, split out of `driver` (500-line budget):
//! builds `(Msg, MsgTag)` pairs, discharges the deferred save/rename
//! `Cmd`s, and runs one `update` call under `catch_unwind`, checking every
//! invariant against the resulting `Snapshot`/`StepCtx`. Nothing here
//! changes behaviour beyond plan WP5's rename discharge — every other
//! function is exactly what `driver` used to define locally, now reached
//! through `step_exec::`.

use rune_tui::app::{self, App};
use rune_tui::db::DbBridge;
use rune_tui::keymap::{self, KeyInput};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::Vfs;

use crate::action::{HighlightVersion, highlight_spans_from_raw};
use crate::guard;
use crate::invariant::{self, Violation};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

use super::store_ops::wait_for_db_op;
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

/// Builds the `(Msg, MsgTag)` pair for a `Msg::Highlighted` reply from its
/// already-built `HighlightReply` — the one place `Action::Highlight` and
/// `Action::HighlightTree` both resolve the version they claim against the
/// LIVE buffer version at delivery time (`HighlightVersion::resolve`'s own
/// docs), never a fixed constant, mirroring `Action::ConfirmTimeout`'s rule.
fn highlight_reply_step(
    state: &State,
    version: HighlightVersion,
    result: rune_tui::highlight::HighlightReply,
    span_count: usize,
) -> (Msg, MsgTag) {
    let live = state.app.active_doc().buffer.version();
    let delivered_version = version.resolve(live);
    let doc = state.app.active;
    let msg = Msg::Highlighted {
        doc,
        version: delivered_version,
        result: Some(result),
    };
    let tag = MsgTag::Highlighted {
        delivered_version,
        span_count,
    };
    (msg, tag)
}

/// Builds the `(Msg, MsgTag)` pair for `Action::Highlight` — hostile span
/// injection into one region's SPAN channel, never a `ParsedTree` — the
/// fuzzer has no way to synthesize one, and shouldn't (`Action::HighlightTree`
/// reaches the tree channel instead, through `highlight_tree_step`). A reply
/// describes the document's whole region layout, so this one is a single
/// span-backed region: no coordinate map (its spans are already buffer
/// offsets, exactly like a markdown fence's), and nothing for the tree
/// channel. The render-path query reads it back the same way it reads any
/// other region, so `HL-CLAMPED`/`HL-STALE-DROP` keep testing the real clamp.
pub(super) fn highlight_step(
    state: &State,
    version: HighlightVersion,
    spans: &[(usize, usize, u16)],
) -> (Msg, MsgTag) {
    let result = rune_tui::highlight::HighlightReply {
        regions: vec![rune_tui::highlight::RegionResult {
            map: rune_tui::linemap::LineMap::default(),
            payload: Some(rune_tui::highlight::RegionPayload::Spans(
                highlight_spans_from_raw(spans),
            )),
        }],
        truncated: false,
    };
    highlight_reply_step(state, version, result, spans.len())
}

/// Builds the `(Msg, MsgTag)` pair for `Action::HighlightTree` — the tree
/// channel `Action::Highlight` cannot reach (that fn's own docs), delivered
/// through `crate::action::highlight_tree_reply` so the driver and its
/// acceptance test share the one construction. `span_count` is 0: the reply
/// carries a `RegionPayload::Tree`, not spans.
pub(super) fn highlight_tree_step(
    state: &State,
    version: HighlightVersion,
    fixture: u8,
    base: usize,
) -> (Msg, MsgTag) {
    let result = crate::action::highlight_tree_reply(fixture, base);
    highlight_reply_step(state, version, result, 0)
}

/// Runs the one deferred save `Cmd`, if any, returning the `Msg` it
/// produced together with its tag — and, for the no-store fallback only,
/// the bytes it was constructed with, looked up in the per-document
/// snapshot by the ack's OWN `id`, never by whichever document is active
/// at delivery time (see `MsgTag::SaveDone`'s docs; `TODO-fuzz-save-
/// verbatim-help-doc-stale-ack.md`). The snapshot is guaranteed to have an
/// entry for `id`: it was built from every open document at the moment
/// THIS save `Cmd` was constructed, and `id` names exactly the document
/// `trigger_save`/`materialize_ack::materialize_vfs_cmd` built the `Cmd`
/// for — which must have existed then. `CmdKind::Save` now covers TWO
/// distinct `Cmd` shapes (WP7's store-backed dance spawns its own caller-
/// side `vfs` `Cmd` under the same tag `step_and_check` already tracks as
/// `pending_save`, alongside the pre-existing no-store fallback): a
/// `Msg::SaveDone` reply carries verbatim bytes worth pinning (`SAVE-
/// VERBATIM`), a `Msg::MaterializeVfsDone` reply does not — `SAVE-VERBATIM`
/// stays scoped to the tag it already keys off. Never synthesizes either
/// reply itself (G14) — this only ever forwards what `cmd.run()` actually
/// returned, and returns `None` for any OTHER reply shape as a defensive
/// guard against `Cmd`'s general contract, not a path reachable from any
/// real save `Cmd` this driver stores.
pub(super) fn discharge_pending_save(state: &mut State) -> Option<(Msg, MsgTag, Option<Vec<u8>>)> {
    let (cmd, per_doc_bytes) = state.pending_save.take()?;
    let msg = cmd.run()?;
    match &msg {
        Msg::SaveDone {
            id,
            version,
            result,
            ..
        } => {
            let tag = MsgTag::SaveDone {
                id: *id,
                version: *version,
                ok: result.is_ok(),
            };
            let bytes = per_doc_bytes.get(id).cloned().unwrap_or_default();
            Some((msg, tag, Some(bytes)))
        }
        Msg::MaterializeVfsDone { id, .. } => {
            let tag = MsgTag::MaterializeVfsDone { id: *id };
            Some((msg, tag, None))
        }
        _ => None,
    }
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

/// Finds the oldest still-pending recovery-store op (lowest id) and blocks
/// on `bridge` for its reply, returning the `(Msg, MsgTag)` pair to deliver
/// — `Action::DeliverDb`'s and the end-of-session sweep's shared drain
/// primitive. `None` when nothing is pending (either no store is wired, or
/// every op issued so far has already been drained). Oldest-first mirrors
/// the real writer thread's own FIFO completion order.
pub(super) fn drain_one_db_op(state: &mut State, bridge: &DbBridge) -> Option<(Msg, MsgTag)> {
    let op_id = *state.app.db_ops.keys().min()?;
    // Read BEFORE the reply is delivered: `handle_db_event` pops this exact
    // entry out of `db_ops` as part of routing the ack (`db_dispatch.rs`),
    // so it's gone from `App` by the time any checker could otherwise ask
    // which document `op_id` belonged to.
    let doc = state.app.db_ops.get(&op_id).map(|pending| pending.doc);
    let evt = wait_for_db_op(bridge, op_id);
    Some((Msg::Db(evt), MsgTag::Db { op_id, doc }))
}

/// Runs the one deferred trash `Cmd`, if any, returning the `Msg` it
/// produced together with its tag (plan WP3.S3) — the same shape as
/// `discharge_pending_rename`, closing the identical driver gap for
/// `CmdKind::Trash`: left undischarged, `Mem::trash` and `Msg::TrashDone`
/// are unreachable from this driver, and fuzz coverage of the trash flow
/// stops at `set_guard`.
pub(super) fn discharge_pending_trash(state: &mut State) -> Option<(Msg, MsgTag)> {
    let cmd = state.pending_trash.take()?;
    let msg = cmd.run()?;
    if !matches!(msg, Msg::TrashDone { .. }) {
        return None;
    }
    Some((msg, MsgTag::TrashDone))
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

    if let MsgTag::Db { doc, .. } = &tag {
        state
            .divergent_save
            .note_prepare_ack(&msg, *doc, step_index);
    }

    let effects = match run_update_catching_panic(&mut state.app, msg) {
        Ok(effects) => effects,
        Err(violation) => {
            outcome.violation = Some(violation);
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
        // Exhaustive over every `CmdKind`: a future variant this driver has
        // no policy for yet fails to COMPILE here rather than falling
        // through and being silently dropped — every kind gets an explicit
        // deferred/dropped classification, forever.
        match cmd.kind() {
            CmdKind::Save => {
                if state.pending_save.is_some() {
                    outcome.violation = Some(Violation::new(
                        "SAVE-SINGLE-FLIGHT",
                        "a second save Cmd arrived while one was already pending \
                                  (G9: at most one save Cmd may ever be outstanding)"
                            .to_string(),
                    ));
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
            }
            CmdKind::Rename => {
                // Structurally at most one at a time (`rename::begin`
                // refuses a second commit while `app.rename.in_flight()`),
                // so overwriting an existing `Some` here can never lose a
                // still-outstanding Cmd in practice — unlike `pending_save`
                // above, there is no separate per-document byte snapshot to
                // carry: no rename-path checker needs one yet.
                state.pending_rename = Some(cmd);
            }
            CmdKind::Trash => {
                // Same single-slot reasoning as `Rename` above: structurally
                // at most one trash `Cmd` can be in flight at a time
                // (`trash::request_trash` and `trash::confirm` both refuse
                // while `app.trash_pending` is `Some`, mirroring `rename::
                // begin`'s `in_flight` refusal), so overwriting an existing
                // `Some` here can never lose a still-outstanding one in
                // practice.
                state.pending_trash = Some(cmd);
            }
            CmdKind::QuitTimeout
            | CmdKind::ClipboardRead
            | CmdKind::SaveConfirmTimeout
            | CmdKind::MessagesCollapseTimeout
            | CmdKind::OpenExternal
            | CmdKind::ReadDir
            | CmdKind::ReadFile
            | CmdKind::Highlight
            | CmdKind::ImageDecode
            | CmdKind::SearchHistory
            | CmdKind::BootstrapView => {
                // Deliberately dropped, each for its own already-documented
                // reason: the four timers/subprocess spawns
                // (`QuitTimeout`/`ClipboardRead`/`SaveConfirmTimeout`/
                // `MessagesCollapseTimeout`) sleep or fork and must never
                // run inline in a headless driver; `OpenExternal` forks
                // `/usr/bin/open` and is unreachable from this driver by
                // construction; `ReadDir`/`ReadFile`/`Highlight`/
                // `ImageDecode`/`SearchHistory`/`BootstrapView` are off-thread
                // reads/parses this driver has no discharge path for yet
                // (their results never reach `update`, so nothing they'd
                // produce is observable either way — `BootstrapView` is also
                // unreachable from this driver by construction, since it is
                // only ever spawned from `runtime::bootstrap`, which this
                // driver never runs). Each arm above IS discharged — this arm
                // is the intentional rest, not an accidental one.
            }
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
    if next.merge_active {
        outcome.merge_activated = true;
    }
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
        active_is_seed_doc: state.app.active == state.seed_doc,
    };

    let mut violation = invariant::check_all(prev, &next, &ctx);
    // The stateful anti-loop tracker sees every checked step in order; a
    // `check_all` violation stopping the session first makes its own state
    // moot, so `or_else` ordering costs nothing.
    if violation.is_none() {
        violation = state.rediverge.observe(prev, &next, &ctx);
    }
    if violation.is_none() {
        violation = state.divergent_save.observe(prev, &next, &ctx);
    }
    // `SYNC-IDEMPOTENT`/`WRAP-RT` need live `&mut App`/`ViewSnapshots`
    // access `Snapshot` can't carry (module docs) — checked only on
    // sampled steps (G19: the display pipeline dominates debug-build
    // runtime).
    if violation.is_none() && sampled {
        violation = match guard::catching_panic(|| {
            checks::sync_idempotent_check(&mut state.app)
                .or_else(|| checks::wrap_rt_check(&state.app, next.line_count))
        }) {
            Ok(checked) => checked,
            Err(panicked) => Some(panicked),
        };
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

/// Wraps one `update` + `sync_view` call — G13's three live
/// `debug_assert!`s (buffer line-starts, undo `reapply`, render cell-map
/// length) fire in this debug build, and any of them (or a genuine panic
/// anywhere in the pipeline) must produce a `NO-PANIC` violation rather
/// than aborting the whole fuzz run.
fn run_update_catching_panic(app: &mut App, msg: Msg) -> Result<Effects, Violation> {
    guard::catching_panic(move || {
        let mut effects = Effects::default();
        app::update(app, msg, &mut effects);
        app.sync_view();
        effects
    })
}
