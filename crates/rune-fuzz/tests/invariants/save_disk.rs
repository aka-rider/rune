use rune_fuzz::invariant::{save_clean_matches_disk, save_no_trailing_ws, save_verbatim};
use rune_fuzz::step::{MsgTag, StepCtx};

use crate::support::{base_active_id, base_ctx, other_doc_id};

#[test]
fn save_verbatim_detects_a_disk_mismatch() {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::SaveDone {
        id: base_active_id(),
        version: 2,
        ok: true,
    };
    ctx.disk = Some(b"on disk".to_vec());
    ctx.delivered_save_bytes = Some(b"what was saved".to_vec());
    let v = save_verbatim(&ctx)
        .expect("disk bytes differing from delivered save bytes must trip SAVE-VERBATIM");
    assert_eq!(v.id, "SAVE-VERBATIM");
}

#[test]
fn save_verbatim_accepts_matching_bytes() {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::SaveDone {
        id: base_active_id(),
        version: 2,
        ok: true,
    };
    ctx.disk = Some(b"exact bytes".to_vec());
    ctx.delivered_save_bytes = Some(b"exact bytes".to_vec());
    assert_eq!(save_verbatim(&ctx), None);
}

#[test]
fn save_verbatim_ignores_a_failed_save_done() {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::SaveDone {
        id: base_active_id(),
        version: 2,
        ok: false,
    };
    ctx.disk = Some(b"whatever was there before".to_vec());
    ctx.delivered_save_bytes = Some(b"different bytes".to_vec());
    assert_eq!(save_verbatim(&ctx), None);
}

fn clean_ctx_after_one_save() -> StepCtx {
    let mut ctx = base_ctx();
    ctx.saves_delivered_ok = 1;
    ctx.pending_save_bytes = None;
    ctx.disk_diverged_since_publish = false;
    ctx
}

#[test]
fn save_clean_matches_disk_detects_a_stale_disk_read() {
    let mut next = crate::support::base_snapshot("current content");
    next.is_dirty = false;
    let mut ctx = clean_ctx_after_one_save();
    ctx.disk = Some(b"stale content".to_vec());
    let v = save_clean_matches_disk(&next, &ctx).expect(
        "a clean doc whose disk bytes don't match content must trip SAVE-CLEAN-MATCHES-DISK",
    );
    assert_eq!(v.id, "SAVE-CLEAN-MATCHES-DISK");
}

#[test]
fn save_clean_matches_disk_accepts_matching_disk() {
    let mut next = crate::support::base_snapshot("current content");
    next.is_dirty = false;
    let mut ctx = clean_ctx_after_one_save();
    ctx.disk = Some(b"current content".to_vec());
    assert_eq!(save_clean_matches_disk(&next, &ctx), None);
}

#[test]
fn save_clean_matches_disk_is_inert_while_dirty() {
    let mut next = crate::support::base_snapshot("current content");
    next.is_dirty = true;
    let mut ctx = clean_ctx_after_one_save();
    ctx.disk = Some(b"unrelated stale bytes".to_vec());
    assert_eq!(save_clean_matches_disk(&next, &ctx), None);
}

#[test]
fn save_clean_matches_disk_is_inert_on_the_read_only_help_document() {
    let mut next = crate::support::base_snapshot("# Help\n\n## Global\n");
    next.is_dirty = false;
    next.read_only = rune_tui::document::ReadOnly::Always;
    let mut ctx = clean_ctx_after_one_save();
    ctx.active_is_seed_doc = false;
    ctx.disk = Some(b"hello world".to_vec());
    assert_eq!(save_clean_matches_disk(&next, &ctx), None);
}

#[test]
fn save_clean_matches_disk_is_inert_on_a_fresh_untitled_after_the_seed_doc_closed() {
    let mut next = crate::support::base_snapshot("");
    next.is_dirty = false;
    next.read_only = rune_tui::document::ReadOnly::No;
    let mut ctx = clean_ctx_after_one_save();
    ctx.active_is_seed_doc = false;
    ctx.disk = Some(b"whatever the closed seed document last held".to_vec());
    assert_eq!(save_clean_matches_disk(&next, &ctx), None);
}

#[test]
fn save_clean_matches_disk_still_catches_a_stale_disk_read_on_a_writable_document() {
    let mut next = crate::support::base_snapshot("current content");
    next.is_dirty = false;
    next.read_only = rune_tui::document::ReadOnly::No;
    let mut ctx = clean_ctx_after_one_save();
    ctx.disk = Some(b"stale content".to_vec());
    let v = save_clean_matches_disk(&next, &ctx)
        .expect("a writable, clean document with a stale disk read must still trip the invariant");
    assert_eq!(v.id, "SAVE-CLEAN-MATCHES-DISK");
}

#[test]
fn save_clean_matches_disk_still_catches_a_later_step_disk_mismatch_with_no_divergence_flagged() {
    let mut next = crate::support::base_snapshot("current content");
    next.is_dirty = false;
    let mut ctx = clean_ctx_after_one_save();
    ctx.msg = MsgTag::Resize(80, 24);
    ctx.disk = Some(b"silently rewritten by something other than DivergeDisk".to_vec());
    let v = save_clean_matches_disk(&next, &ctx).expect(
        "a later step's disk mismatch must still be caught when no external write was flagged",
    );
    assert_eq!(v.id, "SAVE-CLEAN-MATCHES-DISK");
}

#[test]
fn save_clean_matches_disk_is_inert_once_disk_diverged_since_the_last_publish() {
    let mut next = crate::support::base_snapshot("current content");
    next.is_dirty = false;
    let mut ctx = clean_ctx_after_one_save();
    ctx.msg = MsgTag::Resize(80, 24);
    ctx.disk_diverged_since_publish = true;
    ctx.disk = Some(b"fuzz-external-write-1\n".to_vec());
    assert_eq!(save_clean_matches_disk(&next, &ctx), None);
}

#[test]
fn save_clean_matches_disk_is_inert_on_a_foreign_doc_commit_ack_while_seed_disk_diverged() {
    let mut next = crate::support::base_snapshot("current content");
    next.is_dirty = false;
    let mut ctx = clean_ctx_after_one_save();
    ctx.msg = MsgTag::Db {
        op_id: 9,
        doc: Some(other_doc_id()),
        save_committed: true,
    };
    ctx.disk_diverged_since_publish = true;
    ctx.disk = Some(b"fuzz-external-write-1\n".to_vec());
    assert_eq!(save_clean_matches_disk(&next, &ctx), None);
}

fn clean_after_one_save(content: &str, disk: &[u8]) -> (rune_fuzz::snapshot::Snapshot, StepCtx) {
    let mut next = crate::support::base_snapshot(content);
    next.is_dirty = false;
    let mut ctx = clean_ctx_after_one_save();
    ctx.disk = Some(disk.to_vec());
    (next, ctx)
}

#[test]
fn save_no_trailing_ws_detects_a_line_ending_in_a_space() {
    let (next, ctx) = clean_after_one_save("alpha \nbeta\n", b"alpha \nbeta\n");
    let v = save_no_trailing_ws(&next, &ctx)
        .expect("a saved line ending in a space must trip SAVE-NO-TRAILING-WS");
    assert_eq!(v.id, "SAVE-NO-TRAILING-WS");
    assert!(v.message.contains("line 1"), "{}", v.message);
    assert!(v.message.contains("<SP>"), "{}", v.message);
}

#[test]
fn save_no_trailing_ws_detects_a_tab_before_a_crlf() {
    let (next, ctx) = clean_after_one_save("one\r\ntwo\t\r\n", b"one\r\ntwo\t\r\n");
    let v = save_no_trailing_ws(&next, &ctx)
        .expect("a tab before a CRLF must trip SAVE-NO-TRAILING-WS");
    assert!(v.message.contains("line 2"), "{}", v.message);
    assert!(v.message.contains("<TAB>"), "{}", v.message);
}

#[test]
fn save_no_trailing_ws_detects_a_final_line_with_no_terminator() {
    let (next, ctx) = clean_after_one_save("head\ntail  ", b"head\ntail  ");
    let v = save_no_trailing_ws(&next, &ctx)
        .expect("an unterminated final line ending in spaces must trip SAVE-NO-TRAILING-WS");
    assert!(v.message.contains("line 2"), "{}", v.message);
}

#[test]
fn save_no_trailing_ws_accepts_preserved_crlf_terminators() {
    let (next, ctx) = clean_after_one_save("one\r\ntwo\r\n", b"one\r\ntwo\r\n");
    assert_eq!(save_no_trailing_ws(&next, &ctx), None);
}

#[test]
fn save_no_trailing_ws_accepts_a_blank_line_between_two_stripped_lines() {
    let (next, ctx) = clean_after_one_save("one\n\ntwo\n", b"one\n\ntwo\n");
    assert_eq!(save_no_trailing_ws(&next, &ctx), None);
}

#[test]
fn save_no_trailing_ws_is_inert_before_any_save_is_delivered() {
    let (next, mut ctx) = clean_after_one_save("alpha \n", b"alpha \n");
    ctx.saves_delivered_ok = 0;
    assert_eq!(save_no_trailing_ws(&next, &ctx), None);
}

#[test]
fn save_no_trailing_ws_is_inert_while_a_save_is_still_pending() {
    let (next, mut ctx) = clean_after_one_save("alpha \n", b"alpha \n");
    ctx.pending_save_bytes = Some(b"alpha\n".to_vec());
    assert_eq!(save_no_trailing_ws(&next, &ctx), None);
}

#[test]
fn save_no_trailing_ws_is_inert_while_dirty() {
    let (mut next, ctx) = clean_after_one_save("alpha \n", b"alpha \n");
    next.is_dirty = true;
    assert_eq!(save_no_trailing_ws(&next, &ctx), None);
}

#[test]
fn save_no_trailing_ws_is_inert_when_the_active_document_is_not_the_seeded_one() {
    let (next, mut ctx) = clean_after_one_save("", b"alpha \n");
    ctx.active_is_seed_doc = false;
    assert_eq!(save_no_trailing_ws(&next, &ctx), None);
}

#[test]
fn save_no_trailing_ws_is_inert_once_disk_diverged_since_the_last_publish() {
    let (next, mut ctx) = clean_after_one_save("alpha", b"alpha \n");
    ctx.disk_diverged_since_publish = true;
    assert_eq!(save_no_trailing_ws(&next, &ctx), None);
}
