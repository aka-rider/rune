//! The `MergePrep` ack landing — where a fresh-state read
//! either turns into a clean-merge/discard install, a working-form install
//! (entering the resolver), or a refusal, each with user feedback. Reached
//! only through `db_dispatch::handle_db_event`'s `OpOutcome::MergePrep` arm.

use rune_core::buffer::Edit;
use rune_core::cursor::{CursorId, CursorSet};
use rune_core::undo::EditKind;
#[cfg(test)]
use rune_db::BlobHash;
use rune_db::{MergePrepOutcome, MergePrepResult, ObsId, SyncKind};

use crate::app::App;
use crate::commands::edit_core::apply_edit_batch_with_cursors;
use crate::db::PendingOp;
use crate::document::DocumentId;
use crate::messages;
use crate::runtime::Effects;

use super::session::{Block, BlockOrigin, Conflict, ConflictBlock, MergeSession, Resolution};
use super::state::MergeIntent;
use super::state::MergeState;

const UTF8_REFUSAL: &str = "merge unavailable — the file on disk is not valid UTF-8";

/// Routes a `MergePrep` ack. `merge_gen` is the ack's own
/// `PendingOp::merge_gen` — `None` (an ack for a different kind of op that
/// somehow reached this router, which never happens through the real
/// dispatch table) is treated exactly like a stale/mismatched ticket.
pub(crate) fn handle_merge_prep_ack(
    app: &mut App,
    doc: DocumentId,
    merge_gen: Option<crate::generation::Generation>,
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

    if app
        .doc(doc)
        .is_some_and(crate::document::Document::save_in_flight)
    {
        super::set_last_sync(app, doc, prep.sync.kind);
        app.merge = MergeState::Inactive;
        let merge_key = crate::global::label_for(crate::global::GlobalCommand::Merge);
        messages::warn(
            app,
            format!("save in flight — merge cancelled, press {merge_key} once it completes"),
        );
        return;
    }

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
    let MergePrepOutcome::Ready { ancestor, theirs } = prep.outcome else {
        app.merge = MergeState::Inactive;
        messages::error(app, "disk is changing — try again");
        return;
    };

    // Review fix F4: `sync.kind` claiming `DiskAhead`/`Diverged` with no
    // `theirs` version at all is an inconsistency `classify_sync` should
    // never produce — surfaced as a clean refusal, no `0`/empty-`Vec`
    // sentinel standing in for "absent" to unwrap past.
    let Some((theirs_obs, theirs_bytes)) = theirs else {
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

    let ancestor_text = match ancestor {
        None => None,
        Some((_rung, bytes)) => match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(_) => {
                app.merge = MergeState::Inactive;
                messages::error(app, UTF8_REFUSAL);
                return;
            }
        },
    };

    let Some(active) = app.doc(doc) else {
        app.merge = MergeState::Inactive;
        return;
    };
    // Captured fresh, NOW — not whatever `merge::begin` saw when the
    // `MergePrep` op was enqueued: the user may have kept typing during
    // the round trip, so this re-reads the buffer on fresh bytes.
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
    let Ok((buffer_text, pane_theirs_text, mut pairs)) = build_pane_install(&hunks) else {
        app.merge = MergeState::Inactive;
        messages::error(app, UTF8_REFUSAL);
        return;
    };

    let first_start = pairs.first().map_or(0, |p| p.block.range.start);
    if !install_whole_range(app, doc, &buffer_text, first_start) {
        app.merge = MergeState::Inactive;
        messages::error(app, "merge failed — the document could not be updated");
        return;
    }
    let install_pos = app.doc(doc).map_or(0, |d| d.journal.pos());
    let adopted = enqueue_resolve_adopt(app, doc, theirs_obs);

    if pairs.is_empty() {
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

    let saved_display_name = super::install_resolver_display_name(app, doc);

    let unresolved = pairs.len();
    pairs.extend(auto_applied_entries(&ours_text, &buffer_text, &pairs));
    pairs.sort_by_key(|p| p.block.range.start);
    let cur = pairs
        .iter()
        .position(|p| !p.block.resolution.is_resolved())
        .unwrap_or(0);
    super::persist::enqueue_merge_open(
        app,
        doc,
        prep.sync.ancestor.as_ref().and_then(|v| v.obs),
        theirs_obs,
        &buffer_text,
        &pairs,
    );
    crate::diff_view::install_text(app, doc, pane_theirs_text, "disk".to_string());
    app.merge = MergeState::Active {
        doc,
        session: MergeSession {
            conflicts: pairs,
            cur,
            saved_display_name,
            theirs_obs,
            install_pos,
        },
    };
    messages::info(
        app,
        format!("{unresolved} conflict(s) to resolve — {}", super::VERB_HINT),
    );
}

struct MergeUtf8Error;

fn build_pane_install(
    hunks: &[rune_merge::Hunk],
) -> Result<(String, String, Vec<ConflictBlock>), MergeUtf8Error> {
    let mut merged = String::new();
    let mut theirs_doc = String::new();
    let mut pairs = Vec::new();
    for hunk in hunks {
        match hunk {
            rune_merge::Hunk::Clean(bytes) => {
                let text = std::str::from_utf8(bytes).map_err(|_| MergeUtf8Error)?;
                merged.push_str(text);
                theirs_doc.push_str(text);
            }
            rune_merge::Hunk::Conflict { ours, theirs } => {
                let ours = std::str::from_utf8(ours).map_err(|_| MergeUtf8Error)?;
                let theirs = std::str::from_utf8(theirs).map_err(|_| MergeUtf8Error)?;
                let start = merged.len();
                merged.push_str(ours);
                theirs_doc.push_str(theirs);
                pairs.push(ConflictBlock {
                    conflict: Conflict {
                        ours: ours.to_string(),
                        theirs: theirs.to_string(),
                    },
                    block: Block {
                        range: start..merged.len(),
                        resolution: Resolution::Unresolved,
                    },
                    origin: BlockOrigin::Conflict,
                });
            }
        }
    }
    Ok((merged, theirs_doc, pairs))
}

fn auto_applied_entries(
    pre_merge: &str,
    merged: &str,
    conflicts: &[ConflictBlock],
) -> Vec<ConflictBlock> {
    use rune_merge::RegionKind;

    let map = rune_merge::align(pre_merge, merged);
    let mut entries = Vec::new();
    for region in &map.regions {
        if !matches!(region.kind, RegionKind::Changed | RegionKind::RightOnly) {
            continue;
        }
        let range = crate::diff_view::rows::line_byte_range(merged, region.right_lines.clone());
        let clashes = conflicts
            .iter()
            .any(|c| c.block.range.start < range.end && range.start < c.block.range.end);
        if clashes {
            continue;
        }
        let ours_range =
            crate::diff_view::rows::line_byte_range(pre_merge, region.left_lines.clone());
        let ours = pre_merge.get(ours_range).unwrap_or_default().to_string();
        let theirs = merged.get(range.clone()).unwrap_or_default().to_string();
        entries.push(ConflictBlock {
            conflict: Conflict { ours, theirs },
            block: Block {
                range,
                resolution: Resolution::TookTheirs,
            },
            origin: BlockOrigin::AutoApplied,
        });
    }
    entries
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
/// edit. Never routes through `Document::hydrate` (the suspicious-shrink
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
    apply_edit_batch_with_cursors(
        app,
        doc,
        vec![(edit, CursorId::FIRST)],
        &cursors_before,
        EditKind::Other,
        move |_, _| vec![CursorSet::new(cursor_at).primary()],
    )
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
/// Esc-out ^S silently publish a half-resolved working form over the
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
    match db
        .store
        .resolve_adopt(rune_db::DocId(db_id), theirs_obs, None)
    {
        Ok(op_id) => {
            app.db_ops.insert(op_id, PendingOp::new(doc));
            true
        }
        Err(e) => {
            crate::materialize_ack::on_store_failure(app, &e.to_string());
            false
        }
    }
}

/// The terminal-success half of the adoption: the buffer is now genuinely
/// reconciled with `theirs_obs`'s bytes — the current disk content — so the
/// CAS expectation advances with it (the disk-conflict guard's
/// [S]ave-anyway precedent): the invited ^S now passes against the file the
/// merge just read, while a SECOND external write in between still
/// hash-mismatches into a fresh conflict. The epoch bump retires every
/// probe still in flight for this file: their verdicts were computed
/// against the pre-reconciliation journal and would land a fabricated
/// `Diverged` right after the adoption — the re-merge-prompt loop — so the
/// `OpOutcome::Sync` ack handler drops them and re-probes the post-adoption
/// world instead.
pub(super) fn advance_expect_obs(app: &mut App, doc: DocumentId, theirs_obs: ObsId) {
    if let Some(binding) = app.doc_file_binding_mut(doc) {
        binding.expect_obs = Some(theirs_obs);
        binding.baseline_epoch = binding.baseline_epoch.wrapping_add(1);
    }
}

#[cfg(test)]
#[path = "landing_tests.rs"]
mod tests;
