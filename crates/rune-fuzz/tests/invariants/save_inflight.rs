//! `SAVE-INFLIGHT-SM` detection tests — split out of `protocol.rs` (500-line
//! budget) once the store-backed completion arms (`MaterializeVfsDone`,
//! `Db`) were taught to it alongside the pre-existing no-store `SaveDone`
//! ones.

use rune_fuzz::invariant::save_inflight_sm;
use rune_fuzz::step::MsgTag;
use rune_tui::keymap::{Command, KeyCode, Mods};

use crate::support::{base_active_id, base_ctx, base_snapshot, key, other_doc_id, sup};

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

#[test]
fn save_inflight_sm_accepts_clearing_on_materialize_vfs_done_for_this_document() {
    // WP7's caller-side `vfs` `Cmd` settling synchronously (`Missing`, or a
    // local `vfs`/path-disagreement failure) for the SAME document that
    // armed the save.
    let mut prev = base_snapshot("abc");
    prev.save_in_flight = true;
    let next = base_snapshot("abc");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::MaterializeVfsDone {
        id: base_active_id(),
        committed: false,
    };
    assert_eq!(save_inflight_sm(&prev, &next, &ctx), None);
}

#[test]
fn save_inflight_sm_detects_clearing_on_materialize_vfs_done_for_another_document() {
    // G9 rules this out in production (at most one save `Cmd` outstanding,
    // and a `MaterializeVfsDone` always names the document its OWN `Cmd`
    // was built for) — the checker must still catch it if it ever
    // happened, not treat every `MaterializeVfsDone` as a blanket excuse.
    let mut prev = base_snapshot("abc");
    prev.save_in_flight = true;
    let next = base_snapshot("abc");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::MaterializeVfsDone {
        id: other_doc_id(),
        committed: false,
    };
    let v = save_inflight_sm(&prev, &next, &ctx).expect(
        "a MaterializeVfsDone naming a DIFFERENT document must not excuse this document's clear",
    );
    assert_eq!(v.id, "SAVE-INFLIGHT-SM");
}

#[test]
fn save_inflight_sm_accepts_clearing_on_a_db_ack_naming_this_document() {
    // `materialize_ack::handle_materialize_ack` landing for THIS
    // document's own pending `MaterializeRecord` op — `doc` is the driver's
    // own `App::db_ops` lookup, read before the ack is delivered.
    let mut prev = base_snapshot("abc");
    prev.save_in_flight = true;
    let next = base_snapshot("abc");
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Db {
        op_id: 1,
        doc: Some(base_active_id()),
        save_committed: false,
    };
    assert_eq!(save_inflight_sm(&prev, &next, &ctx), None);
}

#[test]
fn save_inflight_sm_accepts_clearing_on_a_db_ack_that_posts_a_store_failure_message() {
    // `on_store_failure`'s whole-store degrade: the `Db` message that
    // actually failed can belong to a DIFFERENT document (or none, a
    // `Fatal`) while still legitimately stranding this document's armed
    // save — evidenced by the "save failed: ..." message it always posts
    // whenever it strands at least one document.
    let mut prev = base_snapshot("abc");
    prev.save_in_flight = true;
    let mut next = base_snapshot("abc");
    next.status =
        "recovery disabled: writer thread died | save failed: writer thread died".to_string();
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Db {
        op_id: 1,
        doc: Some(other_doc_id()),
        save_committed: false,
    };
    assert_eq!(save_inflight_sm(&prev, &next, &ctx), None);
}

#[test]
fn save_inflight_sm_detects_clearing_on_an_unrelated_db_ack() {
    // The checker must not blanket-allow every `Db` message: an ack for a
    // DIFFERENT document with no store-failure message posted is exactly
    // the "silently dropped on an unrelated Db ack" shape it exists to
    // catch.
    let mut prev = base_snapshot("abc");
    prev.save_in_flight = true;
    let next = base_snapshot("abc"); // status unchanged, no failure text
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Db {
        op_id: 7,
        doc: Some(other_doc_id()),
        save_committed: false,
    };
    let v = save_inflight_sm(&prev, &next, &ctx).expect(
        "a Db ack naming an unrelated document with no failure evidence must still trip \
         SAVE-INFLIGHT-SM",
    );
    assert_eq!(v.id, "SAVE-INFLIGHT-SM");
}
