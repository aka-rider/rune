//! Unit tests for the highlight overlay invariants (plan WP7.S8):
//! `HL-CLAMPED`, `HL-STALE-DROP`, `HL-NO-REFLOW`. Same controlled-experiment
//! pattern as every other file here — one hand-built BAD `Snapshot`/pair per
//! checker asserting it fires with the right id, one well-formed companion
//! of the same shape asserting `None`. Every checker is called DIRECTLY,
//! never through `invariant::check_all`, so first-wins ordering can never
//! mask a case.

use rune_fuzz::invariant::{hl_clamped, hl_no_reflow, hl_stale_drop};
use rune_fuzz::step::MsgTag;
use rune_tui::render::Cell;

use crate::support::{base_ctx, base_snapshot, cell};

/// A `MsgTag::Highlighted` `StepCtx` carrying `delivered_version` — the
/// L2 checkers key off this variant specifically.
fn highlighted_ctx(delivered_version: u64) -> rune_fuzz::step::StepCtx {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Highlighted {
        delivered_version,
        span_count: 0,
    };
    ctx
}

// ---------------------------------------------------------------------
// HL-CLAMPED
// ---------------------------------------------------------------------

#[test]
fn hl_clamped_detects_an_inverted_span() {
    let mut snap = base_snapshot("abcdefgh");
    snap.highlight_spans = vec![(5, 3)]; // start >= end
    let v = hl_clamped(&snap).expect("an inverted stored span must trip HL-CLAMPED");
    assert_eq!(v.id, "HL-CLAMPED");
}

#[test]
fn hl_clamped_detects_a_span_past_content_len() {
    let mut snap = base_snapshot("abc");
    snap.highlight_spans = vec![(0, 10)]; // content.len() == 3
    let v = hl_clamped(&snap).expect("a span past content.len() must trip HL-CLAMPED");
    assert_eq!(v.id, "HL-CLAMPED");
}

#[test]
fn hl_clamped_detects_a_mid_char_boundary() {
    let mut snap = base_snapshot("é"); // 2 bytes; offset 1 is mid-char
    snap.highlight_spans = vec![(0, 1)];
    let v = hl_clamped(&snap).expect("a mid-char stored span must trip HL-CLAMPED");
    assert_eq!(v.id, "HL-CLAMPED");
}

#[test]
fn hl_clamped_accepts_a_well_formed_span() {
    let mut snap = base_snapshot("abcdefgh");
    snap.highlight_spans = vec![(0, 3), (3, 8)];
    assert_eq!(hl_clamped(&snap), None);
}

#[test]
fn hl_clamped_accepts_no_spans() {
    let snap = base_snapshot("abcdefgh");
    assert_eq!(hl_clamped(&snap), None);
}

/// The exact shape `make test-fuzz` first caught (recorded here as a
/// permanent regression, not just a design note): a highlight reply is
/// stored for a LONGER buffer, then a later edit (an undo, here) shrinks
/// the buffer without any new highlight completing. `highlight_version`
/// now lags `version` — the KNOWN-stale case WP5.S4's `[R2]` deliberately
/// tolerates (stale colours, never no colours) — so `HL-CLAMPED` must NOT
/// fire even though the stored span is, in isolation, out of bounds for
/// the CURRENT content.
#[test]
fn hl_clamped_accepts_an_out_of_bounds_span_left_over_from_a_shrinking_edit() {
    let mut snap = base_snapshot("short"); // content.len() == 5
    snap.version = 2; // a further edit landed after the highlight was stored
    snap.highlight_version = 1; // the stored spans still describe version 1
    snap.highlight_spans = vec![(0, 40)]; // valid for the old, longer buffer only
    assert_eq!(hl_clamped(&snap), None);
}

// ---------------------------------------------------------------------
// HL-STALE-DROP
// ---------------------------------------------------------------------

#[test]
fn hl_stale_drop_detects_spans_changing_on_a_stale_reply() {
    let mut prev = base_snapshot("abcdefgh");
    prev.version = 2;
    prev.highlight_spans = vec![(0, 3)];
    let mut next = base_snapshot("abcdefgh");
    next.version = 2; // live version has moved past the reply's delivered_version
    next.highlight_spans = vec![(0, 5)]; // changed anyway — bug
    let ctx = highlighted_ctx(1); // delivered_version=1 != next.version=2
    let v = hl_stale_drop(&prev, &next, &ctx)
        .expect("a stale-version reply that still changed spans must trip HL-STALE-DROP");
    assert_eq!(v.id, "HL-STALE-DROP");
}

#[test]
fn hl_stale_drop_accepts_spans_left_untouched_on_a_stale_reply() {
    let mut prev = base_snapshot("abcdefgh");
    prev.version = 2;
    prev.highlight_spans = vec![(0, 3)];
    let mut next = base_snapshot("abcdefgh");
    next.version = 2;
    next.highlight_spans = vec![(0, 3)]; // unchanged, as required
    let ctx = highlighted_ctx(1);
    assert_eq!(hl_stale_drop(&prev, &next, &ctx), None);
}

#[test]
fn hl_stale_drop_accepts_a_live_reply_that_changes_spans() {
    let mut prev = base_snapshot("abcdefgh");
    prev.version = 2;
    prev.highlight_spans = vec![(0, 3)];
    let mut next = base_snapshot("abcdefgh");
    next.version = 2;
    next.highlight_spans = vec![(0, 5)]; // delivered_version matches — a real update
    let ctx = highlighted_ctx(2);
    assert_eq!(hl_stale_drop(&prev, &next, &ctx), None);
}

#[test]
fn hl_stale_drop_ignores_non_highlighted_messages() {
    let mut prev = base_snapshot("abcdefgh");
    prev.highlight_spans = vec![(0, 3)];
    let mut next = base_snapshot("abcdefgh");
    next.highlight_spans = vec![(0, 5)];
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::DirLoaded; // not a Highlighted step
    assert_eq!(hl_stale_drop(&prev, &next, &ctx), None);
}

// ---------------------------------------------------------------------
// HL-NO-REFLOW
// ---------------------------------------------------------------------

#[test]
fn hl_no_reflow_detects_content_changing_on_a_highlighted_step() {
    let prev = base_snapshot("abc");
    let next = base_snapshot("abcd"); // content changed — a highlight reply must never do this
    let ctx = highlighted_ctx(1);
    let v = hl_no_reflow(&prev, &next, &ctx)
        .expect("content changing on a Msg::Highlighted step must trip HL-NO-REFLOW");
    assert_eq!(v.id, "HL-NO-REFLOW");
}

#[test]
fn hl_no_reflow_detects_a_cell_geometry_change() {
    let mut prev = base_snapshot("abc");
    prev.cells = vec![vec![cell('a', 0), cell('b', 1)]];
    let mut next = base_snapshot("abc");
    next.cells = vec![vec![cell('a', 0), cell_wide('b', 1, 2)]]; // width changed
    let ctx = highlighted_ctx(1);
    let v = hl_no_reflow(&prev, &next, &ctx)
        .expect("a cell geometry change on a Msg::Highlighted step must trip HL-NO-REFLOW");
    assert_eq!(v.id, "HL-NO-REFLOW");
}

#[test]
fn hl_no_reflow_accepts_a_pure_style_change() {
    let prev = base_snapshot("abc");
    let next = base_snapshot("abc"); // identical geometry/content, only style may differ
    let ctx = highlighted_ctx(1);
    assert_eq!(hl_no_reflow(&prev, &next, &ctx), None);
}

#[test]
fn hl_no_reflow_ignores_non_highlighted_messages() {
    let prev = base_snapshot("abc");
    let next = base_snapshot("abcd"); // would trip if it were a Highlighted step
    let ctx = base_ctx(); // MsgTag::Resize
    assert_eq!(hl_no_reflow(&prev, &next, &ctx), None);
}

fn cell_wide(ch: char, buf_offset: i64, width: u8) -> Cell {
    let mut c = cell(ch, buf_offset);
    c.width = width;
    c
}
