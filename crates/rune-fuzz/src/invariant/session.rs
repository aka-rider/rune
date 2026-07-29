//! Message/state-machine protocol invariants: `SAVE-INFLIGHT-SM`,
//! `QUIT-CHORD`, `CONFIRM-GEN`. All three need `StepCtx::msg` — `Snapshot`
//! alone can't express "what caused this transition" (plan Context,
//! decision 7 `[fixes B3]`).

use rune_tui::keymap::{Command, KeyCode, QuitKey};

use super::Violation;
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `SAVE-INFLIGHT-SM` — `save_in_flight` goes false->true only on a
/// `Command::Save` key OR a modal-captured `s`/`S` key while the dirty-close
/// Guard is up, and true->false only on `SaveDone` (G9: at most one save
/// `Cmd` is ever outstanding, so its `Msg::SaveDone` can never be ambiguous
/// about which attempt it answers).
///
/// The Guard arm exists because `banner::handle_dirty_close_key`'s `s`/`S`
/// option calls `trigger_save` directly — a modal captures every key at
/// stage 1 of `dispatch::handle_key`, before `keymap::resolve` ever sees it,
/// so this arming path never carries `Command::Save` in its tag. It only
/// exists at all once a modal was already up on the PREVIOUS snapshot
/// (`prev.modal_open`); no other modal kind (`Error`, `RenameCollision`)
/// ever calls `trigger_save`, so requiring `prev.modal_open` here is not a
/// loose stand-in for "was a DirtyClose Guard up" — it just doesn't need to
/// be any tighter, since arming still only ever follows a genuine
/// `trigger_save` call in production regardless of which modal was up.
///
/// `save_in_flight` is doc-scoped (`Document::save_in_flight`, read off
/// `app.active_doc()` at capture time), so it is only meaningful across two
/// snapshots of the SAME active document. Scoped to `prev.active ==
/// next.active` for exactly the reason `VERSION-MONOTONE`/`REDO-CLEAR`
/// already are (those invariants' own docs): switching the active document
/// (e.g. `F1` toggling to the Help virtual document) makes `prev`/`next`
/// describe two UNRELATED documents, and the freshly-active one having no
/// save in flight is not a state-machine transition of the document the
/// save was actually issued against.
pub fn save_inflight_sm(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if prev.active != next.active {
        return None;
    }
    if !prev.save_in_flight && next.save_in_flight {
        let armed_by_save_key = matches!(
            ctx.msg,
            MsgTag::Key {
                command: Some(Command::Save),
                ..
            }
        );
        let armed_by_guard_save_key = prev.modal_open
            && matches!(
                ctx.msg,
                MsgTag::Key { input, .. }
                    if matches!(input.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&'s'))
            );
        if !armed_by_save_key && !armed_by_guard_save_key {
            return Some(Violation {
                id: "SAVE-INFLIGHT-SM",
                message: format!(
                    "save_in_flight went false->true on {:?}, not a Command::Save key \
                     (and no modal was up for a Guard save key either)",
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
/// `should_quit` goes false->true only on the SAME quit chord armed in
/// `prev.pending_quit`. `Msg::Quit` (a real terminal's input stream ending)
/// is out of this checker's domain entirely, not merely an inert arm —
/// `MsgTag` carries no `Quit` variant at all (`step.rs`'s own module
/// docs), since this headless driver can never construct one (CODE-REVIEW.md
/// rune-fuzz finding 15: the previous `MsgTag::Quit => true` arm was
/// unreachable outside its own unit test).
pub fn quit_chord(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if prev.should_quit || !next.should_quit {
        return None;
    }
    let ok = match &ctx.msg {
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
/// must leave it untouched.
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
