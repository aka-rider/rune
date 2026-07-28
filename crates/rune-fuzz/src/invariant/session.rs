//! Message/state-machine protocol invariants: `SAVE-INFLIGHT-SM`,
//! `QUIT-CHORD`, `CONFIRM-GEN`. All three need `StepCtx::msg` — `Snapshot`
//! alone can't express "what caused this transition" (plan Context,
//! decision 7 `[fixes B3]`).

use rune_tui::keymap::{Command, QuitKey};

use super::Violation;
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `SAVE-INFLIGHT-SM` — `save_in_flight` goes false->true only on a
/// `Command::Save` key, and true->false only on `SaveDone` (G9: at most
/// one save `Cmd` is ever outstanding, so its `Msg::SaveDone` can never be
/// ambiguous about which attempt it answers).
pub fn save_inflight_sm(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !prev.save_in_flight && next.save_in_flight {
        let armed_by_save_key = matches!(
            ctx.msg,
            MsgTag::Key {
                command: Some(Command::Save),
                ..
            }
        );
        if !armed_by_save_key {
            return Some(Violation {
                id: "SAVE-INFLIGHT-SM",
                message: format!(
                    "save_in_flight went false->true on {:?}, not a Command::Save key",
                    ctx.msg
                ),
            });
        }
    }
    if prev.save_in_flight && !next.save_in_flight && !matches!(ctx.msg, MsgTag::SaveDone { .. }) {
        return Some(Violation {
            id: "SAVE-INFLIGHT-SM",
            message: format!(
                "save_in_flight went true->false on {:?}, not a SaveDone",
                ctx.msg
            ),
        });
    }
    None
}

/// `QUIT-CHORD` (protocol only, NOT a dirty check — G15: `handle_quit_key`
/// sets `should_quit` regardless of `is_dirty()`; asserting a dirty check
/// here would be an instant false catch on intended Phase-1 behaviour) —
/// `should_quit` goes false->true only on `Msg::Quit`, or on the SAME quit
/// chord armed in `prev.pending_quit`.
pub fn quit_chord(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if prev.should_quit || !next.should_quit {
        return None;
    }
    let ok = match &ctx.msg {
        MsgTag::Quit => true,
        MsgTag::Key {
            input,
            command: Some(Command::QuitConfirm),
        } => match prev.pending_quit {
            Some((armed_key, _)) => QuitKey::from_key(*input) == Some(armed_key),
            None => false,
        },
        _ => false,
    };
    if ok {
        return None;
    }
    Some(Violation {
        id: "QUIT-CHORD",
        message: format!(
            "should_quit went false->true on {:?} with pending_quit={:?}",
            ctx.msg, prev.pending_quit
        ),
    })
}

/// `CONFIRM-GEN` — on `ConfirmTimeout{generation}`, `pending_quit` clears
/// iff `generation` equals the currently armed one; a stale generation
/// must leave it untouched (`app.rs:166-174`).
pub fn confirm_gen(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    let MsgTag::ConfirmTimeout { generation } = &ctx.msg else {
        return None;
    };
    let armed_generation = prev.pending_quit.map(|(_, g)| g);
    let should_clear = armed_generation == Some(*generation);
    let cleared = prev.pending_quit.is_some() && next.pending_quit.is_none();
    let unchanged = prev.pending_quit == next.pending_quit;

    if should_clear && !cleared {
        return Some(Violation {
            id: "CONFIRM-GEN",
            message: format!(
                "ConfirmTimeout{{generation:{generation}}} matched the armed generation but \
                 pending_quit was not cleared (prev={:?} next={:?})",
                prev.pending_quit, next.pending_quit
            ),
        });
    }
    if !should_clear && !unchanged {
        return Some(Violation {
            id: "CONFIRM-GEN",
            message: format!(
                "ConfirmTimeout{{generation:{generation}}} did not match the armed generation \
                 {:?} but pending_quit changed to {:?}",
                prev.pending_quit, next.pending_quit
            ),
        });
    }
    None
}
