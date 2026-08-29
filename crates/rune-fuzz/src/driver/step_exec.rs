use rune_tui::app::{self, App};
use rune_tui::keymap::{self, Command, KeyInput};
use rune_tui::registry::{Availability, CommandId};
use rune_tui::runtime::{Cmd, CmdError, CmdKind, Effects, Msg, RecentsResult};
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
        generation: generation.raw(),
        result: RecentsResult::Palette(result),
    };
    (msg, MsgTag::PaletteRecentsLoaded)
}

pub(super) fn mouse_step(input: rune_tui::pointer::MouseInput) -> (Msg, MsgTag) {
    (Msg::Mouse(input), MsgTag::Mouse(input))
}

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

pub(super) fn highlight_tree_step(
    state: &State,
    version: HighlightVersion,
    fixture: u8,
    base: usize,
) -> (Msg, MsgTag) {
    let result = crate::action::highlight_tree_reply(fixture, base);
    highlight_reply_step(state, version, result, 0)
}

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
    let per_doc_bytes = state
        .app
        .documents
        .iter()
        .map(|(&id, doc)| (id, doc.buffer.content().as_bytes().to_vec()))
        .collect();
    state.pending_save = Some((cmd, per_doc_bytes));
    false
}

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

    let save_parked_before = state.pending_save.is_some();
    let raw_bytes = effects.raw_bytes();
    for cmd in effects.cmds {
        match cmd.kind() {
            CmdKind::Save => {
                if arm_save_cmd(state, prev, outcome, cmd) {
                    return true;
                }
            }
            CmdKind::Rename => {
                state.pending_rename = Some(cmd);
            }
            CmdKind::Trash => {
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
            | CmdKind::BootstrapView
            | CmdKind::ProjectIndex => {}
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
    if violation.is_none() {
        violation = state.rediverge.observe(prev, &next, &ctx);
    }
    if violation.is_none() {
        violation = state.divergent_save.observe(prev, &next, &ctx);
    }
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
        outcome.final_snapshot = Some(Snapshot::capture(&mut state.app, true));
        outcome.final_ctx = Some(ctx);
        outcome.violation = Some(v);
        return true;
    }
    false
}

fn run_update_catching_panic(app: &mut App, msg: Msg) -> Result<Effects, Violation> {
    guard::catching_panic(move || {
        let mut effects = Effects::default();
        app::update(app, msg, &mut effects);
        app.sync_view();
        effects
    })
}
