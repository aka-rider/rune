//! WP6.S5 detection tests: `SAVE-INFLIGHT-SM`, `QUIT-CHORD`,
//! `CONFIRM-GEN`.

use rune_fuzz::invariant::{confirm_gen, quit_chord, save_inflight_sm};
use rune_fuzz::step::MsgTag;
use rune_tui::keymap::{Command, KeyCode, Mods, QuitKey};

use crate::support::{base_ctx, base_snapshot, key, sup};

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
    let v = save_inflight_sm(&prev, &next, &ctx)
        .expect("a modal-captured non-`s` key arming save_in_flight must still trip SAVE-INFLIGHT-SM");
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
    let v = save_inflight_sm(&prev, &next, &ctx)
        .expect("a plain `s` key with no modal up arming save_in_flight must trip SAVE-INFLIGHT-SM");
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
fn save_inflight_sm_accepts_clearing_on_save_done() {
    let mut prev = base_snapshot("abc");
    prev.save_in_flight = true;
    let next = base_snapshot("abc");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::SaveDone {
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

#[test]
fn quit_chord_accepts_msg_quit() {
    let prev = base_snapshot("abc");
    let mut next = base_snapshot("abc");
    next.should_quit = true;
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Quit;
    assert_eq!(quit_chord(&prev, &next, &ctx), None);
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
