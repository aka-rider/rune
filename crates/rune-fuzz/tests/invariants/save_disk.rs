//! WP6.S5 detection tests: `SAVE-VERBATIM`, `SAVE-CLEAN-MATCHES-DISK`.

use rune_fuzz::invariant::{save_clean_matches_disk, save_verbatim};
use rune_fuzz::step::MsgTag;

use crate::support::{base_active_id, base_ctx};

// ---------------------------------------------------------------------
// SAVE-VERBATIM
// ---------------------------------------------------------------------

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

/// TODO-fuzz-save-clean-matches-disk-help-toggle.md's repro: `F1` makes the
/// virtual Help document active — clean by construction, with synthetic
/// content that has nothing to do with `ctx.disk` (the real, seeded
/// document's on-disk bytes). Must not misreport that as a durability
/// defect.
#[test]
fn save_clean_matches_disk_is_inert_on_the_read_only_help_document() {
    let mut next = crate::support::base_snapshot("# Help\n\n## Global\n");
    next.is_dirty = false;
    next.read_only = true;
    let mut ctx = base_ctx();
    ctx.saves_delivered_ok = 1;
    ctx.pending_save_bytes = None;
    ctx.disk = Some(b"hello world".to_vec());
    assert_eq!(save_clean_matches_disk(&next, &ctx), None);
}

/// Same-document coverage must survive the `read_only` gate above: a clean,
/// NOT-read-only document whose disk bytes are stale is still caught.
#[test]
fn save_clean_matches_disk_still_catches_a_stale_disk_read_on_a_writable_document() {
    let mut next = crate::support::base_snapshot("current content");
    next.is_dirty = false;
    next.read_only = false;
    let mut ctx = base_ctx();
    ctx.saves_delivered_ok = 1;
    ctx.pending_save_bytes = None;
    ctx.disk = Some(b"stale content".to_vec());
    let v = save_clean_matches_disk(&next, &ctx)
        .expect("a writable, clean document with a stale disk read must still trip the invariant");
    assert_eq!(v.id, "SAVE-CLEAN-MATCHES-DISK");
}
