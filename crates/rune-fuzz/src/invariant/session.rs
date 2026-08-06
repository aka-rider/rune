//! Message/state-machine protocol invariants: `SAVE-INFLIGHT-SM`,
//! `QUIT-CHORD`, `CONFIRM-GEN`. All three need `StepCtx::msg` — `Snapshot`
//! alone can't express "what caused this transition" (plan Context,
//! decision 7 `[fixes B3]`).

use rune_tui::guard::GuardKind;
use rune_tui::keymap::{Command, KeyCode, QuitKey};

use super::Violation;
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `SAVE-INFLIGHT-SM` — `save_in_flight` goes false->true only on a
/// `Command::Save` key OR a modal-captured `s`/`S` key while the dirty-close
/// Guard is up, and true->false only on one of THREE legitimate
/// completions (G9: at most one save `Cmd` is ever outstanding, so none of
/// them can ever be ambiguous about which attempt it answers): the no-store
/// fallback's own `SaveDone`; a store-backed save's caller-side `vfs` `Cmd`
/// settling synchronously (`MsgTag::MaterializeVfsDone`, `materialize_ack::
/// handle_materialize_vfs_done`'s `Missing`/`Error`/`PathDisagreement`
/// arms — every other outcome instead enqueues a further `Db` op and
/// leaves `save_in_flight` untouched until THAT lands); or the `Db` ack for
/// that further op landing (`materialize_ack::handle_materialize_ack`, or
/// `on_store_failure`'s whole-store degrade stranding this document's
/// still-armed save). Never a blanket allowance for every `Db`/
/// `MaterializeVfsDone` message — see the two arms below for exactly how
/// each is recognized without reaching into either module's private
/// functions.
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
    if prev.save_in_flight && !next.save_in_flight {
        let store_backed_completion = match &ctx.msg {
            // `id` names the document `materialize_ack::materialize_vfs_
            // cmd` built this exact `Cmd` for — G9 rules out any OTHER
            // document's save having armed this one's `save_in_flight`, so
            // matching `next.active` is airtight, not a loose stand-in:
            // every outcome that settles `save_in_flight` synchronously on
            // this message (`Missing`, or a local `vfs`/path-disagreement
            // failure) does so only for `id` itself.
            MsgTag::MaterializeVfsDone { id } => *id == next.active,
            // Two DISTINCT legitimate paths land on `Msg::Db`, so `doc`
            // alone only covers the first: a `MaterializeRecord` ack for
            // THIS document's own pending op (`handle_materialize_ack`) —
            // `doc` is read by the driver straight out of `App::db_ops`
            // before the ack is delivered (`MsgTag::Db`'s own docs), naming
            // the same document production itself routes the ack to. The
            // second — `on_store_failure`'s whole-store degrade — can
            // strand a document OTHER than the one that owns the failing
            // op (every document with a save in flight, store-wide), so
            // `doc` correlation can't recognize it; that function is the
            // ONLY place that ever posts a "save failed: ..." message on a
            // step whose own op belongs to someone else, so a freshly
            // posted one here is that path's own unambiguous fingerprint.
            MsgTag::Db { doc, .. } => {
                *doc == Some(next.active)
                    || (next.status != prev.status && next.status.contains("save failed:"))
            }
            _ => false,
        };
        if !matches!(ctx.msg, MsgTag::SaveDone { .. }) && !store_backed_completion {
            return Some(Violation {
                id: "SAVE-INFLIGHT-SM",
                message: format!(
                    "save_in_flight went true->false on {:?}, not a SaveDone, a \
                     MaterializeVfsDone/Db ack naming this document, or a Db ack that posted a \
                     store-failure message",
                    ctx.msg
                ),
            });
        }
    }
    None
}

/// `QUIT-CHORD` (protocol only, NOT a dirty check — G15: the ordinary
/// two-press chord quits regardless of `is_dirty()` once it's reached that
/// path at all; asserting a dirty check here would be an instant false
/// catch on intended Phase-1 behaviour) — `should_quit` goes false->true
/// only on one of FOUR named transitions (plan WP2 widened this from one;
/// this pass widened the third into two, since it covers two distinct
/// production call sites): the SAME quit chord armed in `prev.pending_quit`;
/// a `DirtyQuit` Guard's `[D]iscard` answer (immediate, no save involved);
/// the quit-save fan-out's LAST outstanding entry retiring because ITS
/// document's save actually completed (`materialize_ack::quit_if_pending`,
/// reached from both the no-store `Msg::SaveDone` ack AND the store-backed
/// `Msg::Db` -> `handle_materialize_ack` route — the fuzz driver can only
/// tag the former today, since no fuzz document ever constructs a `DocDb`,
/// so the latter is verified through the save-lifecycle's own `save_in_
/// flight` transition instead of a message tag); or that same LAST entry
/// retiring because `workspace::close_now` closed the document it was
/// waiting on out from under it (`materialize_ack::retire_quit_wait`,
/// unconditional, no save involved either). `Msg::Quit` (a real terminal's
/// input stream ending) is out of this checker's domain entirely, not
/// merely an inert arm — `MsgTag` carries no `Quit` variant at all
/// (`step.rs`'s own module docs), since this headless driver can never
/// construct one (CODE-REVIEW.md rune-fuzz finding 15: the previous
/// `MsgTag::Quit => true` arm was unreachable outside its own unit test).
///
/// Active-document-switch-safe: `should_quit`/`pending_quit` are `App`-level
/// fields (`app.should_quit`, `app.pending_quit`), never per-document — a
/// document switch between `prev`/`next` cannot change which two documents'
/// facts are being compared, because there's only ever one copy of either
/// field to begin with. The WP2 arms below key off `prev.guard`/`prev.
/// quit_intent_pending`, which are likewise `App`-level; the two save/close
/// retirement arms are keyed per-document instead, off `dirty_by_doc`/
/// `save_in_flight_by_doc`, exactly because a quit-save fan-out can span
/// more than one document.
pub fn quit_chord(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if prev.should_quit || !next.should_quit {
        return None;
    }
    let armed_chord = match &ctx.msg {
        MsgTag::Key {
            input,
            command: Some(Command::QuitConfirm),
        } => match prev.pending_quit {
            Some((armed_key, _)) => QuitKey::from_key(*input) == Some(armed_key),
            None => false,
        },
        _ => false,
    };
    let guard_discard = match &ctx.msg {
        MsgTag::Key { input, .. } => {
            prev.guard
                .as_ref()
                .is_some_and(|(_, kind)| matches!(kind, GuardKind::DirtyQuit))
                && matches!(input.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&'d'))
        }
        _ => false,
    };
    // The quit-save fan-out's own retirement: legitimate only when EVERY
    // entry that dropped out of `prev.quit_intent_pending` this step did so
    // through one of production's two retirement paths for that SAME
    // document — never merely because the map went empty.
    let quit_intent_fully_retired = prev
        .quit_intent_pending
        .as_ref()
        .is_some_and(|p| !p.is_empty())
        && next
            .quit_intent_pending
            .as_ref()
            .is_none_or(|p| p.is_empty());
    let quit_intent_retirement_legitimate = quit_intent_fully_retired
        && prev
            .quit_intent_pending
            .iter()
            .flatten()
            .all(|&(doc, version)| {
                // `materialize_ack::quit_if_pending`'s own path: a
                // successful ack for THIS document at THIS version — the
                // no-store fallback is tagged `MsgTag::SaveDone` directly;
                // the store-backed `Msg::Db` route carries no such tag yet
                // (module docs above), so it's recognized instead by the
                // save lifecycle's own true->false `save_in_flight`
                // transition for the same document, which `begin_save`/
                // `finish_save_ok`/`abandon_save` (the ONLY three writers of
                // that field) guarantee never happens except at a real save
                // completion.
                let save_done_ack = matches!(
                    &ctx.msg,
                    MsgTag::SaveDone { id, version: v, ok: true } if *id == doc && *v == version
                );
                let save_in_flight_dropped = prev
                    .save_in_flight_by_doc
                    .get(&doc)
                    .copied()
                    .unwrap_or(false)
                    && !next
                        .save_in_flight_by_doc
                        .get(&doc)
                        .copied()
                        .unwrap_or(false);
                // `materialize_ack::retire_quit_wait`'s other call site:
                // `workspace::close_now` closed this document out from
                // under the wait — it no longer appears among the live
                // documents at all.
                let closed_out_from_under_it = !next.dirty_by_doc.contains_key(&doc);
                save_done_ack || save_in_flight_dropped || closed_out_from_under_it
            });
    if armed_chord || guard_discard || quit_intent_retirement_legitimate {
        return None;
    }
    Some(Violation {
        id: "QUIT-CHORD",
        message: format!(
            "should_quit went false->true on {:?} with pending_quit={:?}, guard={:?}, \
             quit_intent_pending={:?} (checked: armed chord, DirtyQuit [D]iscard, quit-save \
             fan-out retirement via SaveDone ack / save_in_flight drop / document closed out \
             from under it)",
            ctx.msg, prev.pending_quit, prev.guard, prev.quit_intent_pending
        ),
    })
}

/// `CONFIRM-GEN` — on `ConfirmTimeout{generation}`, `pending_quit` clears
/// iff `generation` equals the currently armed one; a stale generation
/// must leave it untouched.
///
/// Active-document-switch-safe: `pending_quit` is `App`-level (`app.
/// pending_quit`), same reasoning as `quit_chord` above — no per-document
/// scoping for a switch to make ambiguous.
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

/// `GUARD-ANSWERED` (plan WP2's new fact): a key that actually answers a
/// `DirtyQuit` Guard (`Esc`/`s`/`S`/`d`/`D` — the exact alphabet `banner::
/// guard::handle_guard_key` matches) must always leave the app either
/// quitting, mid-save (the quit-save fan-out started at least one), or
/// showing a status that explains why not — NEVER back in the bit-for-bit
/// identical Guard with nothing else changed. That silent "nothing
/// happened" outcome is exactly the pre-WP2 wedge this whole plan exists to
/// remove; a regression would otherwise only surface as a truncated fuzz
/// session (a later, unrelated cluster's stray key finally landing on the
/// same stuck prompt) rather than a named, attributable violation.
///
/// Scoped to a key while `prev.guard` names a `DirtyQuit` prompt
/// specifically — `DirtyClose`/`RenameCollision` answers are unchanged by
/// this plan and have no analogous claim asserted here.
pub fn guard_answered(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    let Some((_, kind)) = &prev.guard else {
        return None;
    };
    if !matches!(kind, GuardKind::DirtyQuit) {
        return None;
    }
    let MsgTag::Key { input, .. } = &ctx.msg else {
        return None;
    };
    let answers = matches!(input.code, KeyCode::Escape)
        || matches!(input.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&'s') || c.eq_ignore_ascii_case(&'d'));
    if !answers {
        return None;
    }
    let nothing_changed = next.guard == prev.guard
        && next.should_quit == prev.should_quit
        && next.quit_intent_pending == prev.quit_intent_pending
        && next.status == prev.status;
    if nothing_changed {
        return Some(Violation {
            id: "GUARD-ANSWERED",
            message: format!(
                "answering the DirtyQuit guard on {:?} left guard/should_quit/quit_intent_pending/\
                 status all unchanged (guard={:?}, status={:?})",
                ctx.msg, prev.guard, prev.status
            ),
        });
    }
    None
}
