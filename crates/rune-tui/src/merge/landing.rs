//! The `MergePrep` ack landing (plan WP3.S6) — where a fresh-state read
//! either turns into a clean-merge/discard install, a working-form install
//! (entering the resolver), or a refusal, each with user feedback. Reached
//! only through `db_dispatch::handle_db_event`'s `OpOutcome::MergePrep` arm.

use rune_core::buffer::Edit;
use rune_core::cursor::Cursor;
use rune_db::{MergePrepResult, ObsId, SyncKind};

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
        // Stale: a later `⌘M` (or some other transition) already moved
        // `App.merge` on from the attempt this ack belongs to.
        return;
    };

    // The authoritative gate (plan Gotchas `[R3]`): `Document.last_sync`
    // only ever gave `merge::begin` a fast hint to refuse on; THIS fresh
    // classification is what actually decides whether there is still
    // anything to merge.
    if !matches!(prep.sync.kind, SyncKind::DiskAhead | SyncKind::Diverged) {
        app.merge = MergeState::Inactive;
        messages::info(app, "file on disk matches — nothing to merge");
        return;
    }

    // Review fix F4: `sync.kind` claiming `DiskAhead`/`Diverged` with no
    // `theirs` version at all is an inconsistency `classify_sync` should
    // never produce — surfaced as a clean refusal (§1.7: no `0`/empty-`Vec`
    // sentinel standing in for "absent" to unwrap past).
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

    let ancestor_text = match &prep.ancestor {
        Some(bytes) => match String::from_utf8(bytes.clone()) {
            Ok(text) => Some(text),
            Err(_) => {
                app.merge = MergeState::Inactive;
                messages::error(app, UTF8_REFUSAL);
                return;
            }
        },
        None => None,
    };

    let Some(active) = app.doc(doc) else {
        app.merge = MergeState::Inactive;
        return;
    };
    // Captured fresh, NOW — not whatever `merge::begin` saw when the
    // `MergePrep` op was enqueued (plan WP3.S6: "Go re-runs on fresh
    // bytes"). The user may have kept typing during the round trip.
    let ours_text = active.buffer.content().to_string();

    let hunks = rune_merge::merge_hunks(
        ancestor_text.as_deref().unwrap_or("").as_bytes(),
        ours_text.as_bytes(),
        theirs_text.as_bytes(),
    );
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
    enqueue_resolve_adopt(app, doc, theirs_obs);

    if blocks.is_empty() {
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
    app.merge = MergeState::Active {
        doc,
        conflicts,
        blocks,
        cur: 0,
        saved_display_name,
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
    enqueue_resolve_adopt(app, doc, theirs_obs);
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

/// Advances `doc`'s CAS baseline to `theirs_obs`, correlated to the install
/// edit's durable seq (plan Gotchas `[B3]`: `edit_seq: None` asks
/// `resolve_adopt` to resolve that seq itself, since the install's own
/// `AppendEdit` ack has not necessarily landed yet — the writer thread's
/// strict FIFO order guarantees it has already been APPLIED, just not yet
/// acknowledged, by the time this op runs).
fn enqueue_resolve_adopt(app: &mut App, doc: DocumentId, theirs_obs: ObsId) {
    let Some(db_id) = app.doc(doc).and_then(|d| d.db.as_ref().map(|db| db.db_id)) else {
        return;
    };
    let Some(db) = app.db.as_ref() else { return };
    if db.degraded {
        return;
    }
    match db.store.resolve_adopt(db_id, theirs_obs, None) {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(doc));
        }
        Err(e) => crate::materialize_ack::on_store_failure(app, e.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
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
            theirs: None,
            theirs_obs: None,
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
