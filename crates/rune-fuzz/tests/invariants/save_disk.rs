//! WP6.S5 detection tests: `SAVE-VERBATIM`, `SAVE-CLEAN-MATCHES-DISK`.

use rune_fuzz::invariant::{save_clean_matches_disk, save_verbatim};
use rune_fuzz::step::MsgTag;

use crate::support::base_ctx;

// ---------------------------------------------------------------------
// SAVE-VERBATIM
// ---------------------------------------------------------------------

#[test]
fn save_verbatim_detects_a_disk_mismatch() {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::SaveDone {
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
        version: 2,
        ok: false,
    };
    ctx.disk = Some(b"whatever was there before".to_vec());
    ctx.delivered_save_bytes = Some(b"different bytes".to_vec());
    assert_eq!(save_verbatim(&ctx), None);
}

// ---------------------------------------------------------------------
// SAVE-CLEAN-MATCHES-DISK
// ---------------------------------------------------------------------

#[test]
fn save_clean_matches_disk_detects_a_stale_disk_read() {
    let mut next = crate::support::base_snapshot("current content");
    next.is_dirty = false;
    let mut ctx = base_ctx();
    ctx.saves_delivered_ok = 1;
    ctx.pending_save_bytes = None;
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
    let mut ctx = base_ctx();
    ctx.saves_delivered_ok = 1;
    ctx.pending_save_bytes = None;
    ctx.disk = Some(b"current content".to_vec());
    assert_eq!(save_clean_matches_disk(&next, &ctx), None);
}

#[test]
fn save_clean_matches_disk_is_inert_while_dirty() {
    let mut next = crate::support::base_snapshot("current content");
    next.is_dirty = true; // e.g. mid UNDO-TOTAL drive, G5
    let mut ctx = base_ctx();
    ctx.saves_delivered_ok = 1;
    ctx.disk = Some(b"unrelated stale bytes".to_vec());
    assert_eq!(save_clean_matches_disk(&next, &ctx), None);
}
