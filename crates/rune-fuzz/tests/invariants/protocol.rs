//! WP6.S5 detection tests: `QUIT-CHORD`, `GUARD-ANSWERED`, `CONFIRM-GEN`.
//! `SAVE-INFLIGHT-SM`'s own tests moved to `save_inflight.rs` (500-line
//! budget) once the store-backed completion arms were taught to it.

use rune_fuzz::invariant::{confirm_gen, guard_answered, quit_chord};
use rune_fuzz::step::MsgTag;
use rune_tui::generation::Generation;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::{Command, KeyCode, Mods, QuitKey};

use crate::support::{base_active_id, base_ctx, base_snapshot, key, other_doc_id, sup};

// ---------------------------------------------------------------------
// QUIT-CHORD
// ---------------------------------------------------------------------

#[test]
fn quit_chord_detects_arming_on_an_unrelated_key() {
    let prev = base_snapshot("abc");
    let mut next = base_snapshot("abc");
    next.should_quit = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('c'), sup()),
        command: Some(Command::Copy),
    };
    let v = quit_chord(&prev, &next, &ctx)
        .expect("should_quit flipping on an unrelated key must trip QUIT-CHORD");
    assert_eq!(v.id, "QUIT-CHORD");
}

#[test]
fn quit_chord_detects_a_mismatched_chord() {
    let mut prev = base_snapshot("abc");
    prev.pending_quit = Some((QuitKey::CtrlC, Generation::ZERO));
    let mut next = base_snapshot("abc");
    next.should_quit = true;
    let ctrl_d = key(
        KeyCode::Char('d'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: ctrl_d,
        command: Some(Command::QuitConfirm),
    };
    let v = quit_chord(&prev, &next, &ctx)
        .expect("a different quit chord than the one armed must trip QUIT-CHORD");
    assert_eq!(v.id, "QUIT-CHORD");
}

#[test]
fn quit_chord_accepts_the_same_chord_pressed_twice() {
    let mut prev = base_snapshot("abc");
    prev.pending_quit = Some((QuitKey::CtrlC, Generation::ZERO));
    let mut next = base_snapshot("abc");
    next.should_quit = true;
    let ctrl_c = key(
        KeyCode::Char('c'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: ctrl_c,
        command: Some(Command::QuitConfirm),
    };
    assert_eq!(quit_chord(&prev, &next, &ctx), None);
}

/// Plan WP2's first widened arm: a `DirtyQuit` Guard's `[D]iscard` answer
/// must be accepted as a legitimate false->true transition, distinct from
/// the ordinary two-press chord.
#[test]
fn quit_chord_accepts_a_dirty_quit_guard_discard_answer() {
    let mut prev = base_snapshot("abc");
    prev.guard = Some((base_active_id(), GuardKind::DirtyQuit));
    let mut next = base_snapshot("abc");
    next.should_quit = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('d'), Mods::NONE),
        command: None,
    };
    assert_eq!(quit_chord(&prev, &next, &ctx), None);
}

/// The converse: an ordinary `d` key with NO `DirtyQuit` Guard up must
/// still trip QUIT-CHORD if `should_quit` somehow flipped — the widened
/// arm must not become a blanket exemption for the letter `d`.
#[test]
fn quit_chord_detects_a_d_key_with_no_guard_up() {
    let prev = base_snapshot("abc");
    let mut next = base_snapshot("abc");
    next.should_quit = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('d'), Mods::NONE),
        command: None,
    };
    let v = quit_chord(&prev, &next, &ctx)
        .expect("a `d` key with no DirtyQuit guard up must still trip QUIT-CHORD");
    assert_eq!(v.id, "QUIT-CHORD");
}

/// Plan WP2's second widened arm: the quit-save fan-out's last outstanding
/// save acking successfully must be accepted.
#[test]
fn quit_chord_accepts_the_final_quit_save_ack() {
    let mut prev = base_snapshot("abc");
    prev.quit_intent_pending = Some(vec![(base_active_id(), 3)]);
    let mut next = base_snapshot("abc");
    next.should_quit = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::SaveDone {
        id: base_active_id(),
        version: 3,
        ok: true,
    };
    assert_eq!(quit_chord(&prev, &next, &ctx), None);
}

/// The converse: a successful `SaveDone` for a document NOT in the pending
/// quit-intent set must not be treated as a legitimate quit completion. The
/// waited-on document (`other_doc_id`) stays open and never had its own
/// save in flight in this snapshot pair — still present in `dirty_by_doc`
/// both before and after, `save_in_flight_by_doc` false throughout — so
/// neither the "document closed out from under it" nor the "its own save
/// completed" retirement path applies; only the (wrong) document's
/// `SaveDone` arrived.
#[test]
fn quit_chord_detects_an_unrelated_save_done_while_should_quit_flips() {
    let mut prev = base_snapshot("abc");
    prev.quit_intent_pending = Some(vec![(other_doc_id(), 3)]);
    prev.dirty_by_doc = [(other_doc_id(), true)].into_iter().collect();
    prev.save_in_flight_by_doc = [(other_doc_id(), false)].into_iter().collect();
    let mut next = base_snapshot("abc");
    next.should_quit = true;
    next.dirty_by_doc = [(other_doc_id(), true)].into_iter().collect();
    next.save_in_flight_by_doc = [(other_doc_id(), false)].into_iter().collect();
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::SaveDone {
        id: base_active_id(),
        version: 3,
        ok: true,
    };
    let v = quit_chord(&prev, &next, &ctx)
        .expect("a SaveDone for a document outside the quit intent must trip QUIT-CHORD");
    assert_eq!(v.id, "QUIT-CHORD");
}

// ---------------------------------------------------------------------
// GUARD-ANSWERED
// ---------------------------------------------------------------------

#[test]
fn guard_answered_detects_discard_leaving_everything_unchanged() {
    let doc = base_active_id();
    let mut prev = base_snapshot("abc");
    prev.guard = Some((doc, GuardKind::DirtyQuit));
    let next = prev.clone();
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('d'), Mods::NONE),
        command: None,
    };
    let v = guard_answered(&prev, &next, &ctx).expect(
        "answering DirtyQuit with `d` and leaving guard/should_quit/status untouched must trip \
         GUARD-ANSWERED",
    );
    assert_eq!(v.id, "GUARD-ANSWERED");
}

#[test]
fn guard_answered_accepts_discard_that_actually_quits() {
    let doc = base_active_id();
    let mut prev = base_snapshot("abc");
    prev.guard = Some((doc, GuardKind::DirtyQuit));
    let mut next = base_snapshot("abc");
    next.should_quit = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('d'), Mods::NONE),
        command: None,
    };
    assert_eq!(guard_answered(&prev, &next, &ctx), None);
}

#[test]
fn guard_answered_accepts_save_that_starts_the_quit_intent() {
    let doc = base_active_id();
    let mut prev = base_snapshot("abc");
    prev.guard = Some((doc, GuardKind::DirtyQuit));
    let mut next = base_snapshot("abc");
    next.quit_intent_pending = Some(vec![(doc, 1)]);
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('s'), Mods::NONE),
        command: None,
    };
    assert_eq!(guard_answered(&prev, &next, &ctx), None);
}

#[test]
fn guard_answered_ignores_keys_outside_the_answer_alphabet() {
    let doc = base_active_id();
    let mut prev = base_snapshot("abc");
    prev.guard = Some((doc, GuardKind::DirtyQuit));
    let next = prev.clone();
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('x'), Mods::NONE),
        command: None,
    };
    assert_eq!(
        guard_answered(&prev, &next, &ctx),
        None,
        "a non-answer key consumed as a no-op is not this invariant's concern"
    );
}

// ---------------------------------------------------------------------
// CONFIRM-GEN
// ---------------------------------------------------------------------

#[test]
fn confirm_gen_detects_a_stale_generation_incorrectly_clearing() {
    let mut prev = base_snapshot("abc");
    prev.pending_quit = Some((QuitKey::CtrlC, Generation::from_raw(1)));
    let next = base_snapshot("abc"); // pending_quit cleared, but generation is stale
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::ConfirmTimeout { generation: 0 };
    let v = confirm_gen(&prev, &next, &ctx)
        .expect("a stale ConfirmTimeout clearing pending_quit must trip CONFIRM-GEN");
    assert_eq!(v.id, "CONFIRM-GEN");
}

#[test]
fn confirm_gen_detects_a_matching_generation_not_clearing() {
    let mut prev = base_snapshot("abc");
    prev.pending_quit = Some((QuitKey::CtrlC, Generation::ZERO));
    let mut next = base_snapshot("abc");
    next.pending_quit = Some((QuitKey::CtrlC, Generation::ZERO)); // should have cleared, didn't
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::ConfirmTimeout { generation: 0 };
    let v = confirm_gen(&prev, &next, &ctx)
        .expect("a matching ConfirmTimeout that fails to clear must trip CONFIRM-GEN");
    assert_eq!(v.id, "CONFIRM-GEN");
}

#[test]
fn confirm_gen_accepts_a_matching_generation_clearing() {
    let mut prev = base_snapshot("abc");
    prev.pending_quit = Some((QuitKey::CtrlC, Generation::ZERO));
    let next = base_snapshot("abc");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::ConfirmTimeout { generation: 0 };
    assert_eq!(confirm_gen(&prev, &next, &ctx), None);
}

#[test]
fn confirm_gen_accepts_a_stale_generation_left_untouched() {
    let mut prev = base_snapshot("abc");
    prev.pending_quit = Some((QuitKey::CtrlC, Generation::from_raw(1)));
    let mut next = base_snapshot("abc");
    next.pending_quit = Some((QuitKey::CtrlC, Generation::from_raw(1))); // unchanged: correct for a stale timeout
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::ConfirmTimeout { generation: 0 };
    assert_eq!(confirm_gen(&prev, &next, &ctx), None);
}
