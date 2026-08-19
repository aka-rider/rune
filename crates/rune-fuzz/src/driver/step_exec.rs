//! One-message step execution, split out of `driver` (500-line budget):
//! builds `(Msg, MsgTag)` pairs, discharges the deferred save/rename
//! `Cmd`s, and runs one `update` call under `catch_unwind`, checking every
//! invariant against the resulting `Snapshot`/`StepCtx`. Nothing here
//! changes behaviour beyond plan WP5's rename discharge — every other
//! function is exactly what `driver` used to define locally, now reached
//! through `step_exec::`.

use rune_tui::app::{self, App};
use rune_tui::keymap::{self, Command, KeyInput};
use rune_tui::registry::{Availability, CommandId};
use rune_tui::runtime::{Cmd, CmdError, CmdKind, Effects, Msg, RecentsKind, RecentsResult};
use rune_vfs::Vfs;

use crate::action::{HighlightVersion, PaletteGenClaim, highlight_spans_from_raw};
use crate::guard;
use crate::invariant::{self, Violation};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

use super::checks;
use super::seed_scope::{tag_delivers_seed_save, tag_publishes_seed_doc};
use super::session::{Outcome, State};

fn palette_pending_save_command(app: &App, key: KeyInput) -> Option<Command> {
    let state = app.palette()?;
    if key.code != keymap::KeyCode::Enter || key.mods != keymap::Mods::NONE {
        return None;
    }
    let row = state.rows.get(state.nav.cursor)?;
    if !matches!(row.availability, Availability::Available) {
        return None;
    }
    if row.id == CommandId::Global(keymap::GlobalCommand::Save) {
        Some(Command::Save)
    } else {
        None
    }
}

pub(super) fn key_step(app: &App, key: KeyInput) -> (Msg, MsgTag) {
    let command = keymap::resolve(key).or_else(|| palette_pending_save_command(app, key));
    let tag = MsgTag::Key {
        input: key,
        command,
    };
    (Msg::Key(key), tag)
}

pub(super) fn palette_recents_step(
    state: &State,
    generation: PaletteGenClaim,
    ok: bool,
    names: Vec<String>,
) -> (Msg, MsgTag) {
    let live = state.app.palette().map(|p| p.generation);
    let generation = generation.resolve(live);
    let result = if ok {
        Ok(names)
    } else {
        Err(CmdError::Refused("fuzz".to_string()))
    };
    let msg = Msg::RecentsLoaded {
        kind: RecentsKind::Palette,
        generation: generation.raw(),
        result: RecentsResult::Strings(result),
    };
    (msg, MsgTag::PaletteRecentsLoaded)
}

/// Builds the `(Msg, MsgTag)` pair for one mouse event — no keymap
/// resolution to consult, unlike `key_step`: `dispatch` routes `Msg::Mouse`
/// straight to `commands::mouse::handle`.
pub(super) fn mouse_step(input: rune_tui::pointer::MouseInput) -> (Msg, MsgTag) {
    (Msg::Mouse(input), MsgTag::Mouse(input))
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
        result: rune_tui::highlight::PassOutcome::Replace(result),
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
            outcome: rune_tui::highlight::RegionOutcome::Replace(
                rune_tui::highlight::RegionPayload::Spans {
                    source: String::new(),
                    spans: highlight_spans_from_raw(spans),
                },
            ),
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

/// Arms `state.pending_save` with a just-produced `CmdKind::Save`, or
/// records `SAVE-SINGLE-FLIGHT` and stops the session (G9: at most one
/// save `Cmd` may ever be outstanding — silently overwriting a still-
/// pending one would drop the first save's `Cmd` on the floor, never
/// delivered, `SAVE-CLEAN-MATCHES-DISK` never even sampling it, CODE-
/// REVIEW.md rune-fuzz finding 3). Returns `true` when the session must
/// stop.
fn arm_save_cmd(state: &mut State, prev: &Snapshot, outcome: &mut Outcome, cmd: Cmd) -> bool {
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
    false
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
    let is_save_done_ok = tag_delivers_seed_save(&tag, state.seed_doc);
    let publishes_seed_doc = tag_publishes_seed_doc(&tag, state.seed_doc);
    if publishes_seed_doc {
        state.disk_diverged_since_publish = false;
    }

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
    let save_parked_before = state.pending_save.is_some();
    let raw_bytes = effects.raw_bytes();
    for cmd in effects.cmds {
        // Exhaustive over every `CmdKind`: a future variant this driver has
        // no policy for yet fails to COMPILE here rather than falling
        // through and being silently dropped — every kind gets an explicit
        // deferred/dropped classification, forever.
        match cmd.kind() {
            CmdKind::Save => {
                if arm_save_cmd(state, prev, outcome, cmd) {
                    return true;
                }
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
            CmdKind::Highlight => {
                state.pending_highlights.push_back(cmd);
            }
            CmdKind::ClipboardRead
            | CmdKind::OpenExternal
            | CmdKind::ReadDir
            | CmdKind::ReadFile
            | CmdKind::ImageDecode
            | CmdKind::ImageEncode
            | CmdKind::SearchHistory
            | CmdKind::BootstrapView => {
                // Deliberately dropped, each for its own already-documented
                // reason: `ClipboardRead` forks `/usr/bin/pbpaste` and must
                // never run inline in a headless driver (the quit-confirm/
                // save-confirm/messages-collapse timeouts are no longer
                // `Cmd`s at all — they arm directly on `App::timers`, whose
                // background thread this driver never `attach`es, so they
                // never fire here either, same as when they were dropped
                // `Cmd`s); `OpenExternal` forks `/usr/bin/open` (reachable
                // via a ctrl-click on a link) and must never fork here;
                // `ReadDir`/`ReadFile`/`ImageDecode`/`SearchHistory`/
                // `BootstrapView` are off-thread
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

    let old_path = state.path.clone();
    if let Some(path) = state
        .app
        .doc(state.seed_doc)
        .and_then(|d| d.file_path.clone())
    {
        state.path = path;
    }
    if state.path != old_path {
        let announced = matches!(
            &tag,
            MsgTag::RenameDone
                | MsgTag::SaveDone { .. }
                | MsgTag::MaterializeVfsDone { .. }
                | MsgTag::Db { .. }
        );
        if !announced {
            outcome.violation = Some(Violation::new(
                "SEED-PATH-SILENT-REBIND",
                format!(
                    "the seed document's own file path changed from {old_path:?} to {:?} on a \
                     step whose tag was neither RenameDone nor a save/materialize ack: {tag:?}",
                    state.path
                ),
            ));
            outcome.final_snapshot = Some(Snapshot::capture(&mut state.app, true));
            outcome.final_ctx = None;
            return true;
        }
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
    let save_newly_parked = !save_parked_before && state.pending_save.is_some();

    let ctx = StepCtx {
        step: step_index,
        msg: tag,
        raw: raw_bytes,
        disk,
        pending_save_bytes,
        save_newly_parked,
        delivered_save_bytes,
        saves_delivered_ok: state.saves_delivered_ok,
        active_is_seed_doc: state.app.active == state.seed_doc,
        disk_diverged_since_publish: state.disk_diverged_since_publish,
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
