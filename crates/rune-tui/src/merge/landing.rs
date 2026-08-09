//! The `MergePrep` ack landing (plan WP3.S6) — where a fresh-state read
//! either turns into a clean-merge/discard install, a working-form install
//! (entering the resolver), or a refusal, each with user feedback. Reached
//! only through `db_dispatch::handle_db_event`'s `OpOutcome::MergePrep` arm.

use rune_core::buffer::Edit;
use rune_core::cursor::Cursor;
use rune_db::{AncestorRung, MergePrepResult, ObsId, SyncKind};

use crate::app::App;
use crate::commands::edit_core::apply_edit_batch_with_cursors;
use crate::commands::nav_scroll;
use crate::db::PendingOp;
use crate::document::DocumentId;
use crate::messages;
use crate::runtime::Effects;

use super::frame::build_marker_buffer;
use super::state::MergeIntent;
use super::state::MergeState;

const UTF8_REFUSAL: &str = "merge unavailable — the file on disk is not valid UTF-8";

/// Routes a `MergePrep` ack (plan WP3.S6). `merge_gen` is the ack's own
/// `PendingOp::merge_gen` — `None` (an ack for a different kind of op that
/// somehow reached this router, which never happens through the real
/// dispatch table) is treated exactly like a stale/mismatched ticket.
pub(crate) fn handle_merge_prep_ack(
    app: &mut App,
    doc: DocumentId,
    merge_gen: Option<u32>,
    prep: MergePrepResult,
    _effects: &mut Effects,
) {
    let ticket = match (&app.merge, merge_gen) {
        (
            MergeState::Pending {
                doc: d,
                generation,
                intent,
            },
            Some(g),
        ) if *d == doc && *generation == g => Some(*intent),
        _ => None,
    };
    let Some(intent) = ticket else {
        // Stale: a later `^M` (or some other transition) already moved
        // `App.merge` on from the attempt this ack belongs to.
        return;
    };

    // The authoritative gate (plan Gotchas `[R3]`): `Document.last_sync`
    // only ever gave `merge::begin` a fast hint to refuse on; THIS fresh
    // classification is what actually decides whether there is still
    // anything to merge.
    if !prep.sync.kind.is_disk_divergent() {
        // The refusal itself carries fresh authoritative news — the hint
        // that invited this attempt was stale. Keep the classification for
        // the chrome instead of dropping it with the refusal, or the
        // banner/hint would go on re-inviting a merge there is nothing
        // left to do.
        super::set_last_sync(app, doc, prep.sync.kind);
        app.merge = MergeState::Inactive;
        messages::info(app, "file on disk matches — nothing to merge");
        return;
    }

    // Task WP-A(2ii): disk kept disagreeing with itself across every
    // bounded re-probe — never serve an unstable/unconfirmed Theirs. Distinct
    // from the F4 refusal below: this is disk actively changing, not a
    // `classify_sync` inconsistency.
    if prep.unstable {
        app.merge = MergeState::Inactive;
        messages::error(app, "disk is changing — try again");
        return;
    }

    // Review fix F4: `sync.kind` claiming `DiskAhead`/`Diverged` with no
    // `theirs` version at all is an inconsistency `classify_sync` should
    // never produce — surfaced as a clean refusal, no `0`/empty-`Vec`
    // sentinel standing in for "absent" to unwrap past.
    let (Some(theirs_bytes), Some(theirs_obs)) = (prep.theirs.clone(), prep.theirs_obs) else {
        app.merge = MergeState::Inactive;
        messages::error(app, "merge unavailable — no disk version to merge against");
        return;
    };

    let Ok(theirs_text) = String::from_utf8(theirs_bytes) else {
        app.merge = MergeState::Inactive;
        messages::error(app, UTF8_REFUSAL);
        return;
    };

    // Review fix F2: Discard installs `theirs` outright — it never reads
    // the ancestor or runs a 3-way merge — so it is checked and dispatched
    // here, BEFORE either of those, rather than after both, so a corrupted
    // ancestor blob can never refuse a Discard that has no use for it.
    if intent == MergeIntent::Discard {
        discard_install(app, doc, &theirs_text, theirs_obs);
        return;
    }

    // `ancestor_rung` is the single source of truth for whether there IS an
    // ancestor to read at all — `Absent` always takes the honest 2-way path
    // below regardless of what `prep.ancestor` happens to hold; `ancestor`
    // itself stays only the bytes carrier for the `Lineage`/`SessionScoped`
    // rungs, decoded here.
    let ancestor_text = match prep.ancestor_rung {
        AncestorRung::Absent => None,
        AncestorRung::Lineage | AncestorRung::SessionScoped => match &prep.ancestor {
            Some(bytes) => match String::from_utf8(bytes.clone()) {
                Ok(text) => Some(text),
                Err(_) => {
                    app.merge = MergeState::Inactive;
                    messages::error(app, UTF8_REFUSAL);
                    return;
                }
            },
            None => None,
        },
    };

    let Some(active) = app.doc(doc) else {
        app.merge = MergeState::Inactive;
        return;
    };
    // Captured fresh, NOW — not whatever `merge::begin` saw when the
    // `MergePrep` op was enqueued (plan WP3.S6: re-runs on fresh
    // bytes). The user may have kept typing during the round trip.
    let ours_text = active.buffer.content().to_string();

    // No ancestor means no shared basis to classify a change against — a
    // 3-way diff with an empty stand-in ancestor cannot tell that at all
    // (every byte of both sides would count as "new", collapsing to one
    // whole-file conflict no matter how much `ours` and `theirs` actually
    // agree). A direct line diff between the two texts is the honest
    // substitute: it localizes on whatever they share, and treats every
    // remaining difference as a conflict since there is no ancestor to
    // say which side is the "real" change.
    let hunks = match &ancestor_text {
        Some(text) => rune_merge::merge_hunks(
            text.as_bytes(),
            ours_text.as_bytes(),
            theirs_text.as_bytes(),
        ),
        None => {
            messages::info(
                app,
                "no saved ancestor for this file — showing all differences as conflicts",
            );
            rune_merge::merge_hunks_no_ancestor(ours_text.as_bytes(), theirs_text.as_bytes())
        }
    };
    let Ok((buffer_text, blocks, conflicts)) = build_marker_buffer(&hunks) else {
        app.merge = MergeState::Inactive;
        messages::error(app, UTF8_REFUSAL);
        return;
    };

    let first_start = blocks.first().map(|b| b.start).unwrap_or(0);
    if !install_whole_range(app, doc, &buffer_text, first_start) {
        app.merge = MergeState::Inactive;
        messages::error(app, "merge failed — the document could not be updated");
        return;
    }
    let adopted = enqueue_resolve_adopt(app, doc, theirs_obs);

    if blocks.is_empty() {
        if adopted {
            // Terminal success: a merge with zero conflicts completed the
            // moment it was installed.
            advance_expect_obs(app, doc, theirs_obs);
        }
        // Compared against the just-installed buffer content, not any
        // pre-ack copy: a merge whose result happens to byte-equal the
        // disk version is `Clean`; anything else strictly extends it.
        let installed_matches_theirs = app
            .doc(doc)
            .is_some_and(|d| d.buffer.content() == theirs_text);
        super::set_last_sync(
            app,
            doc,
            if installed_matches_theirs {
                SyncKind::Clean
            } else {
                SyncKind::BufferAhead
            },
        );
        app.merge = MergeState::Inactive;
        messages::info(app, "merged cleanly — disk changes applied");
        return;
    }

    let file_name = app
        .doc(doc)
        .map(|d| d.file_name().to_string())
        .unwrap_or_default();
    let saved_display_name = app.doc(doc).and_then(|d| d.display_name.clone());
    if let Some(d) = app.doc_mut(doc) {
        d.display_name = Some(format!("{file_name}: editor <-> disk"));
    }

    let unresolved = blocks.len();
    super::persist::enqueue_merge_open(
        app,
        doc,
        prep.sync.ancestor.as_ref().and_then(|v| v.obs),
        theirs_obs,
        &buffer_text,
        &blocks,
        &conflicts,
    );
    app.merge = MergeState::Active {
        doc,
        conflicts,
        blocks,
        cur: 0,
        saved_display_name,
        theirs_obs,
    };
    if let Some(d) = app.doc_mut(doc) {
        nav_scroll::scroll_to_byte_offset(d, first_start);
    }
    messages::info(
        app,
        format!("{unresolved} conflict(s) to resolve — [O]urs [T]heirs [B]oth"),
    );
}

/// The Discard path (plan Assumption A2, review fix F2): installs the fresh
/// disk bytes outright, provably never touching `ancestor` — this function
/// is called before `handle_merge_prep_ack` ever converts or reads it, so a
/// corrupted ancestor blob can never refuse a Discard.
fn discard_install(app: &mut App, doc: DocumentId, theirs_text: &str, theirs_obs: ObsId) {
    if !install_whole_range(app, doc, theirs_text, 0) {
        app.merge = MergeState::Inactive;
        messages::error(app, "merge failed — the document could not be updated");
        return;
    }
    if enqueue_resolve_adopt(app, doc, theirs_obs) {
        // Terminal success: the install itself is the whole resolution.
        advance_expect_obs(app, doc, theirs_obs);
    }
    // The buffer now byte-equals the disk bytes just installed.
    super::set_last_sync(app, doc, SyncKind::Clean);
    app.merge = MergeState::Inactive;
    messages::info(app, "disk changes adopted");
}

/// Replaces `doc`'s ENTIRE live buffer content with `text` as one journaled
/// edit (plan WP3.S6: "install the working form the same way"). Never
/// routes through `Document::hydrate` (plan Gotchas: the suspicious-shrink
/// guard there skips `db::append_edit`, which would leave this install
/// unreplicated to the recovery store). Returns whether it actually
/// applied — `false` (read-only, or `Buffer::apply_edits` itself refusing)
/// must never be followed by `resolve_adopt` (plan Gotchas `[B3]`).
///
/// The single resulting cursor lands at `cursor_at` (review fix F9) — the
/// first conflict's start for a working-form install, or `0` for a
/// no-conflict/Discard install — rather than at the edit's end (the whole
/// document), which `commit_edit_batch`'s generic per-cursor rule would
/// otherwise place it at while the viewport itself scrolls to the first
/// conflict, leaving the cursor and the view disagreeing about where the
/// user landed.
fn install_whole_range(app: &mut App, doc: DocumentId, text: &str, cursor_at: usize) -> bool {
    let Some(document) = app.doc(doc) else {
        return false;
    };
    let cursors_before = document.cursors.clone();
    let old_len = document.buffer.content().len();
    let edit = Edit {
        start: 0,
        end: old_len,
        insert: text.to_string(),
    };
    apply_edit_batch_with_cursors(app, doc, vec![(edit, 0)], cursors_before, move |_, _| {
        vec![Cursor {
            position: cursor_at,
            anchor: cursor_at,
            desired_col: 0,
            id: 0,
        }]
    })
}

/// Records the `origin='resolve'` observation for `theirs_obs`, correlated
/// to the install edit's durable seq (plan Gotchas `[B3]`: `edit_seq: None`
/// asks `resolve_adopt` to resolve that seq itself, since the install's own
/// `AppendEdit` ack has not necessarily landed yet — the writer thread's
/// strict FIFO order guarantees it has already been APPLIED, just not yet
/// acknowledged, by the time this op runs). Returns whether the op was
/// actually enqueued (`false` for a store-less/degraded/failed enqueue).
///
/// Deliberately does NOT advance `DocDb::expect_obs`: recording the
/// adoption happens at resolver ENTRY, before the user has resolved
/// anything, and advancing the save-CAS baseline that early would let an
/// Esc-out ⌘S silently publish a conflict-marker working form over the
/// external disk bytes. The baseline advances only at a TERMINAL success —
/// the caller's Discard/clean-merge arms, or `exit_in_place` on completion.
fn enqueue_resolve_adopt(app: &mut App, doc: DocumentId, theirs_obs: ObsId) -> bool {
    let Some(db_id) = app.doc_db_id(doc) else {
        return false;
    };
    let Some(db) = app.db.as_ref() else {
        return false;
    };
    if db.degraded {
        return false;
    }
    match db.store.resolve_adopt(db_id, theirs_obs, None) {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(doc));
            true
        }
        Err(e) => {
            crate::materialize_ack::on_store_failure(app, e.to_string());
            false
        }
    }
}

/// The terminal-success half of the adoption: the buffer is now genuinely
/// reconciled with `theirs_obs`'s bytes — the current disk content — so the
/// CAS expectation advances with it (the disk-conflict guard's
/// [S]ave-anyway precedent): the invited ⌘S now passes against the file the
/// merge just read, while a SECOND external write in between still
/// hash-mismatches into a fresh conflict.
pub(super) fn advance_expect_obs(app: &mut App, doc: DocumentId, theirs_obs: ObsId) {
    if let Some(binding) = app.doc_file_binding_mut(doc) {
        binding.expect_obs = theirs_obs;
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    /// Review fix F2: `discard_install`'s own signature carries no
    /// ancestor at all — it cannot read what it was never given — so a
    /// Discard always installs `theirs` verbatim, provably independent of
    /// whatever the ancestor blob would (or wouldn't) have decoded to.
    #[test]
    fn discard_install_replaces_the_buffer_with_theirs_verbatim_and_never_touches_ancestor() {
        let mut app = app_with("<<<<<<< editor\nours\n=======\ntheirs\n>>>>>>> disk\n");
        let doc = app.active;

        discard_install(&mut app, doc, "disk replacement\n", 42);

        assert_eq!(app.doc(doc).unwrap().buffer.content(), "disk replacement\n");
        assert_eq!(app.merge, MergeState::Inactive);
        assert_eq!(messages::newest_text(&app), Some("disk changes adopted"));
    }

    /// Review fix F4: `sync.kind` claiming `Diverged` (or `DiskAhead`) with
    /// no `theirs`/`theirs_obs` at all is an inconsistency `classify_sync`
    /// should never produce — the landing must refuse cleanly, with a
    /// status, rather than unwrap past it.
    #[test]
    fn ack_refuses_cleanly_when_sync_claims_diverged_but_theirs_is_absent() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.merge = MergeState::Pending {
            doc,
            generation: 0,
            intent: MergeIntent::Merge,
        };

        let prep = MergePrepResult {
            sync: rune_db::SyncState {
                kind: SyncKind::Diverged,
                ancestor: None,
                ours: rune_db::Version {
                    hash: String::new(),
                    obs: None,
                },
                theirs: None,
            },
            ancestor: None,
            ancestor_rung: rune_db::AncestorRung::Absent,
            theirs: None,
            theirs_obs: None,
            unstable: false,
        };

        let mut effects = Effects::default();
        handle_merge_prep_ack(&mut app, doc, Some(0), prep, &mut effects);

        assert_eq!(app.merge, MergeState::Inactive);
        assert!(
            messages::newest_text(&app)
                .unwrap_or_default()
                .contains("no disk version"),
            "expected the F4 refusal status, got {:?}",
            messages::newest_text(&app)
        );
    }

    fn diverged_prep(theirs: &[u8], theirs_obs: ObsId) -> MergePrepResult {
        MergePrepResult {
            sync: rune_db::SyncState {
                kind: SyncKind::Diverged,
                ancestor: None,
                ours: rune_db::Version {
                    hash: String::new(),
                    obs: None,
                },
                theirs: None,
            },
            ancestor: None,
            ancestor_rung: rune_db::AncestorRung::Absent,
            theirs: Some(theirs.to_vec()),
            theirs_obs: Some(theirs_obs),
            unstable: false,
        }
    }

    /// An absent ancestor (`prep.ancestor: None`) must be presented
    /// honestly — a message-pane notice explaining there is no saved
    /// ancestor — and must not degrade to a whole-file conflict: the
    /// no-ancestor 2-way path still localizes on whatever `ours` and
    /// `theirs` share.
    #[test]
    fn absent_ancestor_notifies_and_localizes_via_the_2way_path() {
        let mut app = app_with("shared-start\nours-only\nshared-end\n");
        let doc = app.active;
        app.merge = MergeState::Pending {
            doc,
            generation: 0,
            intent: MergeIntent::Merge,
        };

        let mut effects = Effects::default();
        handle_merge_prep_ack(
            &mut app,
            doc,
            Some(0),
            diverged_prep(b"shared-start\ntheirs-only\nshared-end\n", 3),
            &mut effects,
        );

        assert!(
            messages::log_text(&app).contains("no saved ancestor"),
            "expected the absent-ancestor notice, got {:?}",
            messages::log_text(&app)
        );
        let MergeState::Active { blocks, .. } = &app.merge else {
            panic!("expected an Active merge, got {:?}", app.merge);
        };
        assert_eq!(
            blocks.len(),
            1,
            "expected exactly one localized conflict, not a whole-file collapse"
        );
        let buffer = app.doc(doc).unwrap().buffer.content().to_string();
        assert!(
            buffer.starts_with("shared-start\n"),
            "clean prefix lost: {buffer:?}"
        );
        assert!(
            buffer.ends_with("shared-end\n"),
            "clean suffix lost: {buffer:?}"
        );
    }

    /// A "nothing to merge" refusal still carries a fresh authoritative
    /// classification — the landing must keep it on `last_sync`, or the
    /// stale hint that invited the attempt would re-invite it forever.
    #[test]
    fn nothing_to_merge_refusal_records_the_fresh_classification() {
        let mut app = app_with("hello");
        let doc = app.active;
        app.merge = MergeState::Pending {
            doc,
            generation: 0,
            intent: MergeIntent::Merge,
        };
        let prep = MergePrepResult {
            sync: rune_db::SyncState {
                kind: SyncKind::Clean,
                ancestor: None,
                ours: rune_db::Version {
                    hash: String::new(),
                    obs: None,
                },
                theirs: None,
            },
            ancestor: None,
            ancestor_rung: rune_db::AncestorRung::Absent,
            theirs: None,
            theirs_obs: None,
            unstable: false,
        };

        let mut effects = Effects::default();
        handle_merge_prep_ack(&mut app, doc, Some(0), prep, &mut effects);

        assert_eq!(app.merge, MergeState::Inactive);
        assert_eq!(app.doc(doc).unwrap().last_sync, Some(SyncKind::Clean));
    }

    /// A successful Discard leaves the buffer byte-equal to the disk bytes
    /// it just installed — `last_sync` must say `Clean`.
    #[test]
    fn discard_install_success_marks_the_document_clean() {
        let mut app = app_with("ours");
        let doc = app.active;

        discard_install(&mut app, doc, "theirs\n", 0);

        assert_eq!(app.doc(doc).unwrap().last_sync, Some(SyncKind::Clean));
        assert_eq!(app.merge, MergeState::Inactive);
    }

    /// A failed install (a read-only document refuses the whole-range edit)
    /// must leave both the CAS baseline and the sync classification exactly
    /// as they were — nothing was reconciled, so nothing may claim it was.
    #[test]
    fn failed_install_leaves_expect_obs_and_last_sync_untouched() {
        let mut app = app_with("hello");
        let doc = app.active;
        if let Some(d) = app.doc_mut(doc) {
            d.read_only = crate::document::ReadOnly::Always;
            d.db = Some(crate::db::DocDb::new(1, false, 0));
        }
        app.bind_file(1, 7);
        app.merge = MergeState::Pending {
            doc,
            generation: 0,
            intent: MergeIntent::Merge,
        };

        let mut effects = Effects::default();
        handle_merge_prep_ack(
            &mut app,
            doc,
            Some(0),
            diverged_prep(b"disk\n", 9),
            &mut effects,
        );

        assert_eq!(app.merge, MergeState::Inactive);
        assert_eq!(app.doc(doc).unwrap().buffer.content(), "hello");
        assert_eq!(app.doc(doc).unwrap().last_sync, None);
        assert_eq!(app.file_binding(1).unwrap().expect_obs, 7);
        assert!(
            messages::newest_text(&app)
                .unwrap_or_default()
                .contains("merge failed"),
            "expected the failed-install status, got {:?}",
            messages::newest_text(&app)
        );
    }

    /// `begin` refuses while a save's materialize dance is still in flight
    /// for the document — a `MergePrep` ack landing after the save's commit
    /// ack would rebase the CAS baseline backwards.
    #[test]
    fn begin_refuses_while_a_save_is_in_flight() {
        let mut app = app_with("hello");
        let doc = app.active;
        if let Some(d) = app.doc_mut(doc) {
            d.last_sync = Some(SyncKind::Diverged);
            d.save_in_flight = true;
        }

        let mut effects = Effects::default();
        crate::merge::begin(&mut app, MergeIntent::Merge, &mut effects);

        assert_eq!(app.merge, MergeState::Inactive);
        assert!(
            messages::newest_text(&app)
                .unwrap_or_default()
                .contains("save in progress"),
            "expected the save-in-flight refusal, got {:?}",
            messages::newest_text(&app)
        );
    }

    /// Review fix F9: the install's single resulting cursor lands at
    /// `cursor_at`, not at the edit's end (the whole document) —
    /// `install_whole_range`'s own contract.
    #[test]
    fn install_whole_range_places_the_cursor_at_the_requested_offset() {
        let mut app = app_with("old content");
        let doc = app.active;

        assert!(install_whole_range(&mut app, doc, "new content", 4));

        let cursors = app.doc(doc).unwrap().cursors.all();
        assert_eq!(cursors.len(), 1);
        assert_eq!(cursors[0].position, 4);
        assert_eq!(cursors[0].anchor, 4);
    }
}
