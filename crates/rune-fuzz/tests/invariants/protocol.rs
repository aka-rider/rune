//! WP6.S5 detection tests: `SAVE-INFLIGHT-SM`, `QUIT-CHORD`,
//! `CONFIRM-GEN`.

use rune_fuzz::invariant::{confirm_gen, guard_answered, quit_chord, save_inflight_sm};
use rune_fuzz::step::MsgTag;
use rune_tui::banner::GuardKind;
use rune_tui::keymap::{Command, KeyCode, Mods, QuitKey};

use crate::support::{base_active_id, base_ctx, base_snapshot, key, other_doc_id, sup};

// ---------------------------------------------------------------------
// SAVE-INFLIGHT-SM
// ---------------------------------------------------------------------

#[test]
fn save_inflight_sm_detects_arming_without_a_save_command() {
    let prev = base_snapshot("abc");
    let mut next = base_snapshot("abc");
    next.save_in_flight = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('c'), sup()),
        command: Some(Command::Copy),
    };
    let v = save_inflight_sm(&prev, &next, &ctx)
        .expect("save_in_flight arming on a non-Save key must trip SAVE-INFLIGHT-SM");
    assert_eq!(v.id, "SAVE-INFLIGHT-SM");
}

#[test]
fn save_inflight_sm_detects_clearing_without_save_done() {
    let mut prev = base_snapshot("abc");
    prev.save_in_flight = true;
    let next = base_snapshot("abc"); // save_in_flight now false
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('c'), sup()),
        command: Some(Command::Copy),
    };
    let v = save_inflight_sm(&prev, &next, &ctx)
        .expect("save_in_flight clearing on a non-SaveDone message must trip SAVE-INFLIGHT-SM");
    assert_eq!(v.id, "SAVE-INFLIGHT-SM");
}

#[test]
fn save_inflight_sm_accepts_arming_on_a_modal_captured_save_key() {
    // `banner::handle_dirty_close_key`'s `s`/`S` option calls `trigger_save`
    // directly — a modal captures the key at stage 1 of `dispatch::
    // handle_key`, before `keymap::resolve` ever runs, so this tag never
    // carries `Command::Save`.
    let mut prev = base_snapshot("abc");
    prev.modal_open = true;
    let mut next = base_snapshot("abc");
    next.modal_open = true;
    next.save_in_flight = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('s'), Mods::NONE),
        command: None,
    };
    assert_eq!(save_inflight_sm(&prev, &next, &ctx), None);
}

#[test]
fn save_inflight_sm_detects_a_modal_captured_non_save_key_arming() {
    let mut prev = base_snapshot("abc");
    prev.modal_open = true;
    let mut next = base_snapshot("abc");
    next.modal_open = false; // e.g. `d`/`D` cleared the modal
    next.save_in_flight = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('d'), Mods::NONE),
        command: None,
    };
    let v = save_inflight_sm(&prev, &next, &ctx).expect(
        "a modal-captured non-`s` key arming save_in_flight must still trip SAVE-INFLIGHT-SM",
    );
    assert_eq!(v.id, "SAVE-INFLIGHT-SM");
}

#[test]
fn save_inflight_sm_detects_an_s_key_arming_with_no_modal_up() {
    let prev = base_snapshot("abc");
    let mut next = base_snapshot("abc");
    next.save_in_flight = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('s'), Mods::NONE),
        command: None,
    };
    let v = save_inflight_sm(&prev, &next, &ctx).expect(
        "a plain `s` key with no modal up arming save_in_flight must trip SAVE-INFLIGHT-SM",
    );
    assert_eq!(v.id, "SAVE-INFLIGHT-SM");
}

#[test]
fn save_inflight_sm_accepts_arming_on_a_save_command() {
    let prev = base_snapshot("abc");
    let mut next = base_snapshot("abc");
    next.save_in_flight = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('s'), sup()),
        command: Some(Command::Save),
    };
    assert_eq!(save_inflight_sm(&prev, &next, &ctx), None);
}

#[test]
fn save_inflight_sm_accepts_clearing_on_an_active_document_switch() {
    // Repro: type into a doc, `⌘S` (arms save_in_flight on that document),
    // keep typing with the save still outstanding, then `F1` — which
    // swaps `app.active` to the virtual Help document. `save_in_flight`
    // is doc-scoped (`Snapshot::capture` reads it off `app.active_doc()`),
    // so the freshly-active Help document naturally reports no save in
    // flight; that's not a state-machine transition of the document the
    // save was actually issued against, and must NOT trip SAVE-INFLIGHT-SM.
    let mut prev = base_snapshot("hello world");
    prev.save_in_flight = true;
    let mut next = base_snapshot("hello world");
    next.active = other_doc_id();
    next.save_in_flight = false;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::F1, Mods::NONE),
        command: None,
    };
    assert_eq!(save_inflight_sm(&prev, &next, &ctx), None);
}

#[test]
fn save_inflight_sm_still_detects_a_same_document_false_flip() {
    // Same false-clear as above, but WITHOUT an active-document switch:
    // the gate must not swallow a genuine same-document violation.
    let mut prev = base_snapshot("hello world");
    prev.save_in_flight = true;
    let mut next = base_snapshot("hello world");
    next.save_in_flight = false;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::F1, Mods::NONE),
        command: None,
    };
    let v = save_inflight_sm(&prev, &next, &ctx).expect(
        "save_in_flight clearing on a non-SaveDone message with the SAME active document \
         must still trip SAVE-INFLIGHT-SM",
    );
    assert_eq!(v.id, "SAVE-INFLIGHT-SM");
}

#[test]
fn save_inflight_sm_accepts_clearing_on_save_done() {
    let mut prev = base_snapshot("abc");
    prev.save_in_flight = true;
    let next = base_snapshot("abc");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::SaveDone {
        id: base_active_id(),
        version: 2,
        ok: true,
    };
    assert_eq!(save_inflight_sm(&prev, &next, &ctx), None);
}

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
    prev.pending_quit = Some((QuitKey::CtrlC, 0));
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
    prev.pending_quit = Some((QuitKey::CtrlC, 0));
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
/// quit-intent set must not be treated as a legitimate quit completion.
#[test]
fn quit_chord_detects_an_unrelated_save_done_while_should_quit_flips() {
    let mut prev = base_snapshot("abc");
    prev.quit_intent_pending = Some(vec![(other_doc_id(), 3)]);
    let mut next = base_snapshot("abc");
    next.should_quit = true;
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
    prev.pending_quit = Some((QuitKey::CtrlC, 1));
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
    prev.pending_quit = Some((QuitKey::CtrlC, 0));
    let mut next = base_snapshot("abc");
    next.pending_quit = Some((QuitKey::CtrlC, 0)); // should have cleared, didn't
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::ConfirmTimeout { generation: 0 };
    let v = confirm_gen(&prev, &next, &ctx)
        .expect("a matching ConfirmTimeout that fails to clear must trip CONFIRM-GEN");
    assert_eq!(v.id, "CONFIRM-GEN");
}

#[test]
fn confirm_gen_accepts_a_matching_generation_clearing() {
    let mut prev = base_snapshot("abc");
    prev.pending_quit = Some((QuitKey::CtrlC, 0));
    let next = base_snapshot("abc");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::ConfirmTimeout { generation: 0 };
    assert_eq!(confirm_gen(&prev, &next, &ctx), None);
}

#[test]
fn confirm_gen_accepts_a_stale_generation_left_untouched() {
    let mut prev = base_snapshot("abc");
    prev.pending_quit = Some((QuitKey::CtrlC, 1));
    let mut next = base_snapshot("abc");
    next.pending_quit = Some((QuitKey::CtrlC, 1)); // unchanged: correct for a stale timeout
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::ConfirmTimeout { generation: 0 };
    assert_eq!(confirm_gen(&prev, &next, &ctx), None);
}
