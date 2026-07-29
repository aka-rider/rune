//! `PANE-NO-BLEED` — the rule the `UNDO-TOTAL`/`REDO-TOTAL` harness fix
//! (`driver.rs::restore_editor_focus`) rests on, pinned as an invariant so
//! a future change toward Go's behaviour (`workspace_update_keys.go`
//! Priority 2.5 bleeding an unfocused pane's keystroke into the document
//! as an invisible edit) is caught by the fuzzer instead of landing
//! silently. Needs `StepCtx.msg` (L2): a keystroke aimed at chrome — the
//! Explorer or the Open Tabs pane, with no modal capturing it first — must
//! never mutate the active document behind it.

use ratatui::layout::Rect;
use rune_tui::pane::Pane;

use super::{Violation, trunc};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// Fires when, on a `MsgTag::Key` step, `prev` had no modal up, focus
/// somewhere other than `Pane::Editor`, and the SAME active document
/// before and after — yet that document's `content`/`version`/
/// `journal_len` changed anyway.
///
/// The `prev.active == next.active` guard is what keeps this free of
/// false positives: every non-editor key path that DOES change what a
/// `Snapshot` observes also changes the active document (Explorer `Enter`
/// opening a file via `workspace::open_path`; Tabs `Enter` via
/// `switch_to`; `^w` closing the active tab via `close_now`) — this
/// checker simply never fires on those, because it's scoped to the
/// no-active-document-change case. The paths that keep the active
/// document unchanged (`⌘S`, a focus/toggle chord, a quit chord, a failed
/// open raising `Modal::Error`, `^w` arming the Guard, closing a
/// non-active tab) never touch a buffer byte either, so they're silent
/// here too.
///
/// Scoped to `MsgTag::Key` deliberately: `Msg::Paste`/`Msg::ClipboardRead`
/// insert into `app.active` regardless of focus (`app.rs:347-356`), and
/// the driver synthesizes `ClipboardRead` unprompted — async replies are
/// out of this invariant's domain, same reasoning as `clip_osc52`'s own
/// module docs.
pub fn pane_no_bleed(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    if !matches!(ctx.msg, MsgTag::Key { .. }) {
        return None;
    }
    if prev.modal_open || prev.focus == Pane::Editor {
        return None;
    }
    if prev.active != next.active {
        return None;
    }
    let content_changed = prev.content != next.content;
    let version_changed = prev.version != next.version;
    let journal_changed = prev.journal_len != next.journal_len;
    if !content_changed && !version_changed && !journal_changed {
        return None;
    }
    Some(Violation {
        id: "PANE-NO-BLEED",
        message: format!(
            "a key aimed at {:?} (no modal up, active document unchanged) mutated the \
             document: content {:?} -> {:?}, version {} -> {}, journal_len {} -> {}",
            prev.focus,
            trunc(&prev.content, 80),
            trunc(&next.content, 80),
            prev.version,
            next.version,
            prev.journal_len,
            next.journal_len
        ),
    })
}

/// True when `inner` lies entirely inside `outer`. A zero-area `inner` is
/// the placeholder every collapsed pane rect uses (`layout::geometry` hands
/// back `Rect::new(0, 0, 0, 0)` for a section that isn't shown) and is
/// nowhere, not somewhere out of bounds — a plain corner comparison would
/// only pass it by the coincidence of `outer` itself starting at the
/// origin, so it is excluded from the containment check outright rather
/// than relying on that coincidence.
fn within(inner: Rect, outer: Rect) -> bool {
    if inner.width == 0 || inner.height == 0 {
        return true;
    }
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

/// True when `a` and `b` overlap. A zero-area rect never overlaps anything
/// — it is nowhere, not somewhere on top of another pane — so this must be
/// checked ahead of `Rect::intersects`, which (like `within`) only treats a
/// zero-area rect at the origin as harmless by coincidence.
fn overlaps(a: Rect, b: Rect) -> bool {
    if a.width == 0 || a.height == 0 || b.width == 0 || b.height == 0 {
        return false;
    }
    a.intersects(b)
}

/// `LAYOUT-FITS` — every rect `layout::geometry` hands `render::draw` must
/// stay inside the frame it was computed for, and the panes it carves out of
/// the left column must never overlap each other or spill past the block
/// that borders them. Checked on every step (`check_all`, not a sampled
/// checker like `SYNC-IDEMPOTENT`) specifically so the fuzzer's
/// `Action::Resize` storm — which already drives the frame down to 1x1 and
/// up to 200x60 — exercises it at every size, including the degenerate ones
/// a user-draggable splitter newly makes reachable.
pub fn layout_fits(next: &Snapshot) -> Option<Violation> {
    let geo = &next.geometry;

    // The frame `geometry` was computed for is not itself a field of
    // `Geometry` — reconstruct it from the three rects that partition it
    // exactly: `main` (post-banner) plus whatever the banner and footer
    // splits carved off. Both splits partition their input rect with no gap
    // and no overlap, so the union reconstructs the original frame exactly.
    let frame = geo
        .main
        .union(geo.footer)
        .union(geo.banner.unwrap_or_default());

    let mut rects: Vec<(&str, Rect)> = vec![
        ("footer", geo.footer),
        ("explorer_inner", geo.explorer_inner),
        ("tabs_inner", geo.tabs_inner),
        ("center", geo.center),
        ("editor", geo.editor),
        ("main", geo.main),
    ];
    if let Some(r) = geo.banner {
        rects.push(("banner", r));
    }
    if let Some(r) = geo.left_block {
        rects.push(("left_block", r));
    }
    if let Some(r) = geo.tabs_divider {
        rects.push(("tabs_divider", r));
    }
    if let Some(r) = geo.title {
        rects.push(("title", r));
    }
    if let Some(r) = geo.left_splitter {
        rects.push(("left_splitter", r));
    }

    for (name, rect) in &rects {
        if !within(*rect, frame) {
            return Some(Violation {
                id: "LAYOUT-FITS",
                message: format!("{name} {rect:?} does not lie inside the frame {frame:?}"),
            });
        }
    }

    if let Some(left_block) = geo.left_block
        && overlaps(left_block, geo.center)
    {
        return Some(Violation {
            id: "LAYOUT-FITS",
            message: format!("left_block {left_block:?} overlaps center {:?}", geo.center),
        });
    }

    // The three left-column sections, `tabs_divider` only when it exists
    // this frame. Named directly rather than indexed into a slice — there
    // are exactly three of them and every pair is checked explicitly, so no
    // indexing operation can ever be out of bounds.
    let mut column_rects: Vec<(&str, Rect)> = vec![
        ("explorer_inner", geo.explorer_inner),
        ("tabs_inner", geo.tabs_inner),
    ];
    if let Some(r) = geo.tabs_divider {
        column_rects.push(("tabs_divider", r));
    }
    for (i, (name_a, rect_a)) in column_rects.iter().enumerate() {
        for (name_b, rect_b) in column_rects.iter().skip(i + 1) {
            if overlaps(*rect_a, *rect_b) {
                return Some(Violation {
                    id: "LAYOUT-FITS",
                    message: format!(
                        "{name_a} {rect_a:?} overlaps {name_b} {rect_b:?} inside the left column"
                    ),
                });
            }
        }
        if let Some(left_block) = geo.left_block
            && !within(*rect_a, left_block)
        {
            return Some(Violation {
                id: "LAYOUT-FITS",
                message: format!(
                    "{name_a} {rect_a:?} does not lie inside left_block {left_block:?}"
                ),
            });
        }
    }

    if geo.footer.intersects(geo.main) {
        return Some(Violation {
            id: "LAYOUT-FITS",
            message: format!("footer {:?} overlaps main {:?}", geo.footer, geo.main),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `layout::geometry` always hands back the zero rect for a collapsed
    /// section, regardless of where the frame itself starts. Today the
    /// outer frame always starts at the origin too, which is exactly why the
    /// old plain corner comparison got away with treating the zero rect as
    /// contained — this pins the containment/overlap checks against a
    /// left column placed at a NON-zero origin, so the zero-rect placeholder
    /// is never mistaken for one that's merely out of bounds.
    #[test]
    fn collapsed_section_at_a_non_zero_origin_frame_reports_no_violation() {
        let frame = Rect::new(5, 3, 40, 20);
        let left_block = Rect::new(5, 3, 22, 20);
        let collapsed = Rect::new(0, 0, 0, 0);

        assert!(
            within(collapsed, frame),
            "a collapsed rect is nowhere, not out of bounds"
        );
        assert!(within(collapsed, left_block));
        assert!(
            !overlaps(collapsed, left_block),
            "a collapsed rect can never overlap a real one"
        );
        assert!(!overlaps(left_block, collapsed));
    }
}
