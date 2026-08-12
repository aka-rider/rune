//! Unit tests for the merge-mode invariants: `MERGE-DOC-ACTIVE`,
//! `MERGE-SAVE-BLOCKED`, `MERGE-KEY-FEEDBACK`, `MERGE-TITLE-CLEARED`. Same
//! controlled-experiment pattern as every other file here — one hand-built
//! BAD `Snapshot`/pair per checker asserting it fires with the right id,
//! one well-formed companion of the same shape asserting `None`. Every
//! checker is called DIRECTLY, never through `invariant::check_all`, so
//! first-wins ordering can never mask a case.

use rune_core::coords::DisplayRow;
use rune_fuzz::invariant::{
    RedivergenceTracker, merge_doc_active, merge_key_feedback, merge_save_blocked,
    merge_title_cleared,
};
use rune_fuzz::step::MsgTag;
use rune_tui::keymap::{Command, KeyCode, Mods};
use rune_tui::pane::Pane;

use crate::support::{base_active_id, base_ctx, base_snapshot, key, other_doc_id};

fn save_key_ctx() -> rune_fuzz::step::StepCtx {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(
            KeyCode::Char('s'),
            Mods {
                sup: true,
                ..Mods::NONE
            },
        ),
        command: Some(Command::Save),
    };
    ctx
}

fn plain_key_ctx() -> rune_fuzz::step::StepCtx {
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Char('o'), Mods::NONE),
        command: None,
    };
    ctx
}

// ---------------------------------------------------------------------
// MERGE-DOC-ACTIVE
// ---------------------------------------------------------------------

#[test]
fn merge_doc_active_detects_the_merge_doc_no_longer_open() {
    let doc = other_doc_id();
    let mut snap = base_snapshot("abc");
    snap.merge_active = true;
    snap.merge_doc = Some(doc);
    snap.active = doc;
    // `dirty_by_doc` left empty: the merge document isn't among open docs.
    let v = merge_doc_active(&snap)
        .expect("Active naming a closed document must trip MERGE-DOC-ACTIVE");
    assert_eq!(v.id, "MERGE-DOC-ACTIVE");
}

#[test]
fn merge_doc_active_detects_the_merge_doc_not_being_active() {
    let doc = other_doc_id();
    let mut snap = base_snapshot("abc");
    snap.merge_active = true;
    snap.merge_doc = Some(doc);
    snap.active = base_active_id(); // different from `doc`
    snap.dirty_by_doc.insert(doc, false);
    let v = merge_doc_active(&snap)
        .expect("Active naming a non-active document must trip MERGE-DOC-ACTIVE");
    assert_eq!(v.id, "MERGE-DOC-ACTIVE");
}

#[test]
fn merge_doc_active_accepts_the_merge_doc_open_and_active() {
    let doc = other_doc_id();
    let mut snap = base_snapshot("abc");
    snap.merge_active = true;
    snap.merge_doc = Some(doc);
    snap.active = doc;
    snap.dirty_by_doc.insert(doc, false);
    assert_eq!(merge_doc_active(&snap), None);
}

#[test]
fn merge_doc_active_ignores_an_inactive_merge() {
    let snap = base_snapshot("abc"); // merge_active: false by default
    assert_eq!(merge_doc_active(&snap), None);
}

// ---------------------------------------------------------------------
// MERGE-SAVE-BLOCKED
// ---------------------------------------------------------------------

#[test]
fn merge_save_blocked_detects_a_pending_save() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.merge_unresolved = 1;
    let next = base_snapshot("abc");
    let mut ctx = save_key_ctx();
    ctx.pending_save_bytes = Some(b"abc".to_vec());
    let v = merge_save_blocked(&prev, &next, &ctx).expect(
        "a Save key arming pending_save_bytes while unresolved must trip MERGE-SAVE-BLOCKED",
    );
    assert_eq!(v.id, "MERGE-SAVE-BLOCKED");
}

#[test]
fn merge_save_blocked_detects_save_in_flight() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.merge_unresolved = 1;
    let mut next = base_snapshot("abc");
    next.save_in_flight = true;
    let ctx = save_key_ctx();
    let v = merge_save_blocked(&prev, &next, &ctx)
        .expect("a Save key flipping save_in_flight while unresolved must trip MERGE-SAVE-BLOCKED");
    assert_eq!(v.id, "MERGE-SAVE-BLOCKED");
}

#[test]
fn merge_save_blocked_accepts_a_refused_save() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.merge_unresolved = 1;
    let next = base_snapshot("abc");
    let ctx = save_key_ctx(); // pending_save_bytes: None, save_in_flight: false
    assert_eq!(merge_save_blocked(&prev, &next, &ctx), None);
}

#[test]
fn merge_save_blocked_ignores_a_fully_resolved_merge() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.merge_unresolved = 0; // nothing left to block
    let mut next = base_snapshot("abc");
    next.save_in_flight = true;
    let ctx = save_key_ctx();
    assert_eq!(merge_save_blocked(&prev, &next, &ctx), None);
}

#[test]
fn merge_save_blocked_ignores_a_non_save_key() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.merge_unresolved = 1;
    let mut next = base_snapshot("abc");
    next.save_in_flight = true;
    let ctx = plain_key_ctx();
    assert_eq!(merge_save_blocked(&prev, &next, &ctx), None);
}

// ---------------------------------------------------------------------
// MERGE-KEY-FEEDBACK
// ---------------------------------------------------------------------

#[test]
fn merge_key_feedback_detects_a_silent_swallow() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.focus = Pane::Editor;
    let next = prev.clone(); // nothing at all changed
    let ctx = plain_key_ctx();
    let v = merge_key_feedback(&prev, &next, &ctx)
        .expect("a key that changed nothing and set no status must trip MERGE-KEY-FEEDBACK");
    assert_eq!(v.id, "MERGE-KEY-FEEDBACK");
}

#[test]
fn merge_key_feedback_accepts_a_status_change() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.focus = Pane::Editor;
    let mut next = prev.clone();
    next.status = "merge: [O]urs [T]heirs [B]oth · [ ] navigate · Esc close".to_string();
    let ctx = plain_key_ctx();
    assert_eq!(merge_key_feedback(&prev, &next, &ctx), None);
}

#[test]
fn merge_key_feedback_accepts_a_scroll_change() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.focus = Pane::Editor;
    let mut next = prev.clone();
    next.scroll_row = DisplayRow(3);
    let ctx = plain_key_ctx();
    assert_eq!(merge_key_feedback(&prev, &next, &ctx), None);
}

#[test]
fn merge_key_feedback_accepts_a_message_posted_with_identical_status() {
    // Two consecutive unbound keys post the SAME hint text: `status`
    // (footer + newest-entry text) looks unchanged, but the log itself
    // grew by one entry — `message_posts` is the only field that tells the
    // two apart.
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.focus = Pane::Editor;
    prev.status = "merge: [O]urs [T]heirs [B]oth · [ ] navigate · Esc close".to_string();
    let mut next = prev.clone();
    next.message_posts = prev.message_posts + 1;
    let ctx = plain_key_ctx();
    assert_eq!(merge_key_feedback(&prev, &next, &ctx), None);
}

#[test]
fn merge_key_feedback_detects_truly_identical_including_posts() {
    // Same as the silent-swallow case above, but pinned specifically on
    // `message_posts` also matching: this is the case the counter must
    // NOT paper over — a checker that always treated "posts differ" as
    // "feedback happened" without also requiring them to differ would
    // lose its teeth entirely.
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.focus = Pane::Editor;
    prev.message_posts = 5;
    let mut next = prev.clone();
    next.message_posts = 5;
    let ctx = plain_key_ctx();
    let v = merge_key_feedback(&prev, &next, &ctx).expect(
        "identical message_posts alongside everything else must still trip MERGE-KEY-FEEDBACK",
    );
    assert_eq!(v.id, "MERGE-KEY-FEEDBACK");
}

/// Issue #54: a bare `Up` is `viewport_scroll`'s own vocabulary — a scroll
/// request the resolver honours, and a clamped scroll is silent by
/// universal editor convention, so leaving every observable unchanged is
/// exempt rather than a violation.
#[test]
fn merge_key_feedback_exempts_a_bare_scroll_key_on_the_active_merge_doc() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.focus = Pane::Editor;
    prev.merge_doc = Some(prev.active);
    let next = prev.clone(); // clamped at the top: nothing observable moves
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Up, Mods::NONE),
        command: None,
    };
    assert_eq!(merge_key_feedback(&prev, &next, &ctx), None);
}

/// A chord `viewport_scroll` refuses (`⌥⌘↑`, `AddCursorAbove`) is an
/// ordinary editor command with no meaning mid-merge — it must still trip
/// the invariant when left with no observable trace at all.
#[test]
fn merge_key_feedback_still_fires_on_a_modifier_arrow_chord() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.focus = Pane::Editor;
    prev.merge_doc = Some(prev.active);
    let next = prev.clone();
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(
            KeyCode::Up,
            Mods {
                alt: true,
                sup: true,
                ..Mods::NONE
            },
        ),
        command: None,
    };
    let v = merge_key_feedback(&prev, &next, &ctx)
        .expect("a refused modifier chord left with no trace must still trip MERGE-KEY-FEEDBACK");
    assert_eq!(v.id, "MERGE-KEY-FEEDBACK");
}

/// The exemption is scoped to the merge document actually being active: when
/// merge is `Active` on some OTHER document, the key never reaches
/// `intercept` at all, so a silent bare `Up` here is still a genuine defect.
#[test]
fn merge_key_feedback_still_fires_on_a_bare_scroll_key_when_merge_doc_is_not_active() {
    let doc = other_doc_id();
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.focus = Pane::Editor;
    prev.merge_doc = Some(doc); // NOT prev.active
    let next = prev.clone();
    let mut ctx = base_ctx();
    ctx.msg = MsgTag::Key {
        input: key(KeyCode::Up, Mods::NONE),
        command: None,
    };
    let v = merge_key_feedback(&prev, &next, &ctx).expect(
        "a bare Up while merge is Active on a non-active document must still trip \
         MERGE-KEY-FEEDBACK",
    );
    assert_eq!(v.id, "MERGE-KEY-FEEDBACK");
}

#[test]
fn merge_key_feedback_ignores_a_non_editor_focus() {
    let mut prev = base_snapshot("abc");
    prev.merge_active = true;
    prev.focus = Pane::Explorer;
    let next = prev.clone();
    let ctx = plain_key_ctx();
    assert_eq!(merge_key_feedback(&prev, &next, &ctx), None);
}

#[test]
fn merge_key_feedback_ignores_an_inactive_merge() {
    let prev = base_snapshot("abc"); // merge_active: false
    let next = prev.clone();
    let ctx = plain_key_ctx();
    assert_eq!(merge_key_feedback(&prev, &next, &ctx), None);
}

// ---------------------------------------------------------------------
// MERGE-TITLE-CLEARED
// ---------------------------------------------------------------------

#[test]
fn merge_title_cleared_detects_a_stale_retitle() {
    let doc = other_doc_id();
    let mut snap = base_snapshot("abc"); // merge_active/merge_pending: false
    snap.display_name_by_doc
        .insert(doc, Some("notes.md: editor <-> disk".to_string()));
    let v = merge_title_cleared(&snap)
        .expect("a stale retitle surviving merge Inactive must trip MERGE-TITLE-CLEARED");
    assert_eq!(v.id, "MERGE-TITLE-CLEARED");
}

#[test]
fn merge_title_cleared_accepts_a_restored_name() {
    let doc = other_doc_id();
    let mut snap = base_snapshot("abc");
    snap.display_name_by_doc.insert(doc, None);
    assert_eq!(merge_title_cleared(&snap), None);
}

#[test]
fn merge_title_cleared_ignores_an_active_merge() {
    let doc = other_doc_id();
    let mut snap = base_snapshot("abc");
    snap.merge_active = true;
    snap.display_name_by_doc
        .insert(doc, Some("notes.md: editor <-> disk".to_string()));
    assert_eq!(merge_title_cleared(&snap), None);
}

#[test]
fn merge_title_cleared_ignores_a_pending_merge() {
    let doc = other_doc_id();
    let mut snap = base_snapshot("abc");
    snap.merge_pending = true;
    snap.display_name_by_doc
        .insert(doc, Some("notes.md: editor <-> disk".to_string()));
    assert_eq!(merge_title_cleared(&snap), None);
}

// ---------------------------------------------------------------------
// MERGE-NO-INSTANT-REDIVERGENCE (the stateful tracker)
// ---------------------------------------------------------------------

/// The `(prev, next)` pair of a merge completing on the active document:
/// `Active` retires to fully `Inactive` with a reconciled `BufferAhead`
/// classification — the transition that arms the tracker.
fn completion_pair() -> (rune_fuzz::snapshot::Snapshot, rune_fuzz::snapshot::Snapshot) {
    let mut prev = base_snapshot("merged");
    prev.merge_active = true;
    prev.merge_doc = Some(base_active_id());
    prev.merge_unresolved = 1;
    let mut next = base_snapshot("merged");
    next.active_last_sync = Some(rune_db::SyncKind::BufferAhead);
    (prev, next)
}

/// A later step whose snapshot re-classifies the same document `Diverged`.
fn rediverged_after(completed: &rune_fuzz::snapshot::Snapshot) -> rune_fuzz::snapshot::Snapshot {
    let mut next = completed.clone();
    next.active_last_sync = Some(rune_db::SyncKind::Diverged);
    next
}

#[test]
fn redivergence_tracker_detects_diverged_after_completion_with_no_external_write() {
    let mut tracker = RedivergenceTracker::default();
    let (prev, completed) = completion_pair();
    assert_eq!(tracker.observe(&prev, &completed, &base_ctx()), None);

    let rediverged = rediverged_after(&completed);
    let v = tracker
        .observe(&completed, &rediverged, &base_ctx())
        .expect("Diverged with no external write since completion must trip the tracker");
    assert_eq!(v.id, "MERGE-NO-INSTANT-REDIVERGENCE");
}

#[test]
fn redivergence_tracker_accepts_diverged_after_an_external_write() {
    let mut tracker = RedivergenceTracker::default();
    let (prev, completed) = completion_pair();
    assert_eq!(tracker.observe(&prev, &completed, &base_ctx()), None);

    tracker.note_external_write();
    let rediverged = rediverged_after(&completed);
    assert_eq!(tracker.observe(&completed, &rediverged, &base_ctx()), None);
}

#[test]
fn redivergence_tracker_accepts_diverged_after_an_undo_unwound_the_reconciliation() {
    let mut tracker = RedivergenceTracker::default();
    let (prev, completed) = completion_pair();
    assert_eq!(tracker.observe(&prev, &completed, &base_ctx()), None);

    let mut before_undo = completed.clone();
    before_undo.journal_pos = 5;
    let mut unwound = completed.clone();
    unwound.journal_pos = 4;
    assert_eq!(tracker.observe(&before_undo, &unwound, &base_ctx()), None);

    let rediverged = rediverged_after(&unwound);
    assert_eq!(tracker.observe(&unwound, &rediverged, &base_ctx()), None);
}

#[test]
fn redivergence_tracker_never_arms_on_an_escape_out_still_diverged() {
    let mut tracker = RedivergenceTracker::default();
    let (prev, mut escaped_out) = completion_pair();
    escaped_out.active_last_sync = Some(rune_db::SyncKind::Diverged);
    assert_eq!(tracker.observe(&prev, &escaped_out, &base_ctx()), None);

    let still_diverged = escaped_out.clone();
    assert_eq!(
        tracker.observe(&escaped_out, &still_diverged, &base_ctx()),
        None,
        "an Esc-out left truthfully Diverged must never arm the tracker"
    );
}

#[test]
fn redivergence_tracker_accepts_a_reconciled_classification_staying_put() {
    let mut tracker = RedivergenceTracker::default();
    let (prev, completed) = completion_pair();
    assert_eq!(tracker.observe(&prev, &completed, &base_ctx()), None);
    let still_reconciled = completed.clone();
    assert_eq!(
        tracker.observe(&completed, &still_reconciled, &base_ctx()),
        None
    );
}
