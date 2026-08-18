//! `execute_op`'s arm bodies, grouped by the natural clusters they fall
//! into (journal/edit ops, sync/merge ops, materialize/rename ops,
//! document-lifecycle ops) — `writer.rs`'s own match stays a thin routing
//! table into these. A signature that would otherwise grow past seven
//! parameters bundles its extra fields into a small args struct, the same
//! idiom `load_anchor::LoadContext` already uses.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use rusqlite::Connection;

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;
use rune_vfs::{Stat, Vfs};

use crate::Error;
use crate::ids::{DocId, ObsId, SessionId};
use crate::materialize::{MaterializeOutcome, MaterializeTarget};
use crate::retry;
use crate::store::LivenessCheckFn;
use crate::writer::DocUndoState;
use crate::writer_ops::{LoadSource, OpOutcome};

// ---------------------------------------------------------------------
// Edit ops: the journal itself (append, undo-position resolution, the
// snapshot-autosave anchor).
// ---------------------------------------------------------------------

#[cfg(test)]
pub(crate) fn noop(conn: &mut Connection) -> Result<OpOutcome, Error> {
    retry::with_retry(conn, |_tx| Ok(()))?;
    Ok(OpOutcome::None)
}

pub(crate) struct AppendEditArgs {
    pub(crate) session_id: SessionId,
    pub(crate) now: SystemTime,
    pub(crate) doc_id: DocId,
    pub(crate) edits: Vec<AppliedEdit>,
    pub(crate) cursors_before: Vec<Cursor>,
    pub(crate) cursors_after: Vec<Cursor>,
}

pub(crate) fn append_edit(
    conn: &mut Connection,
    undo_state: &mut HashMap<DocId, DocUndoState>,
    args: AppendEditArgs,
) -> Result<OpOutcome, Error> {
    let seq = retry::with_retry(conn, |tx| {
        crate::journal::append_edit(
            tx,
            args.session_id,
            args.now,
            args.doc_id,
            &args.edits,
            &args.cursors_before,
            &args.cursors_after,
        )
    })?;
    // A real edit batch (never empty — see `db_enqueue::append_edit`'s
    // caller) always lands a genuine row, so `seq` is always > 0
    // here; recording it extends this doc's local-position mapping
    // by exactly one entry, matching the ONE local `Journal::push`
    // this `AppendEdit` replicates. With coalescing gone, every
    // `append_edit` call lands a fresh row, so `seq` is now always
    // strictly greater than the previous entry — a violation would
    // mean the journal grew a coalescing path again without
    // updating this mapping.
    let state = undo_state.entry(args.doc_id).or_default();
    state.push_seq(args.doc_id, seq);
    Ok(OpOutcome::Seq(seq))
}

pub(crate) fn move_undo_pos(
    conn: &mut Connection,
    undo_state: &HashMap<DocId, DocUndoState>,
    session_id: SessionId,
    doc_id: DocId,
    local_pos: i64,
) -> Result<OpOutcome, Error> {
    let target_seq = undo_state
        .get(&doc_id)
        .and_then(|state| state.resolve(local_pos))
        .ok_or_else(|| {
            Error::NotFound(format!(
                "move_undo_pos: doc {doc_id} has no durable seq for local position {local_pos}"
            ))
        })?;
    retry::with_retry(conn, |tx| {
        crate::journal::move_undo_pos(tx, session_id, doc_id, target_seq)
    })?;
    Ok(OpOutcome::None)
}

pub(crate) fn create_snapshot(
    conn: &mut Connection,
    session_id: SessionId,
    now: SystemTime,
    doc_id: DocId,
    content: String,
) -> Result<OpOutcome, Error> {
    let row_id = retry::with_retry(conn, |tx| {
        // Resolved fresh, inside the same transaction as the insert
        // — see `OpKind::CreateSnapshot`'s own doc comment.
        let seq = crate::journal::current_seq(tx, session_id, doc_id)?;
        crate::snapshot::create_snapshot(tx, session_id, now, doc_id, &content, seq)
    })?;
    Ok(OpOutcome::SnapshotRowId(row_id))
}

// ---------------------------------------------------------------------
// Sync/merge ops: the probe every disk-divergence decision is built on,
// and the merge-mode fresh-state read/progress/close it shares a shape
// with.
// ---------------------------------------------------------------------

pub(crate) fn probe(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: SessionId,
    doc_id: DocId,
    now: SystemTime,
) -> Result<OpOutcome, Error> {
    let state = crate::probe::probe(conn, vfs, session_id, doc_id, now)?;
    Ok(OpOutcome::Sync(Box::new(state)))
}

pub(crate) fn merge_prep(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: SessionId,
    doc_id: DocId,
    now: SystemTime,
) -> Result<OpOutcome, Error> {
    let result = crate::merge_prep::merge_prep(conn, vfs, session_id, doc_id, now)?;
    Ok(OpOutcome::MergePrep(Box::new(result)))
}

pub(crate) struct MergeOpenArgs {
    pub(crate) session_id: SessionId,
    pub(crate) liveness_check: LivenessCheckFn,
    pub(crate) doc_id: DocId,
    pub(crate) base_obs: Option<ObsId>,
    pub(crate) theirs_obs: ObsId,
    pub(crate) marker_content: String,
    pub(crate) blocks_json: String,
    pub(crate) now: SystemTime,
}

pub(crate) fn merge_open(conn: &mut Connection, args: MergeOpenArgs) -> Result<OpOutcome, Error> {
    crate::merge_state::merge_open(
        conn,
        args.liveness_check.as_ref(),
        crate::merge_state::MergeOpenArgs {
            doc_id: args.doc_id,
            session_id: args.session_id,
            base_obs: args.base_obs,
            theirs_obs: args.theirs_obs,
            marker_content: &args.marker_content,
            blocks_json: &args.blocks_json,
        },
        args.now,
    )?;
    Ok(OpOutcome::None)
}

pub(crate) fn merge_progress(
    conn: &mut Connection,
    liveness_check: &LivenessCheckFn,
    doc_id: DocId,
    session_id: SessionId,
    marker_content: &str,
    blocks_json: &str,
) -> Result<OpOutcome, Error> {
    crate::merge_state::merge_progress(
        conn,
        liveness_check.as_ref(),
        doc_id,
        session_id,
        marker_content,
        blocks_json,
    )?;
    Ok(OpOutcome::None)
}

pub(crate) fn merge_close(
    conn: &mut Connection,
    session_id: SessionId,
    doc_id: DocId,
    state: crate::merge_state::MergeCloseState,
) -> Result<OpOutcome, Error> {
    crate::merge_state::merge_close(conn, doc_id, session_id, state)?;
    Ok(OpOutcome::None)
}

// ---------------------------------------------------------------------
// Materialize ops: the save bookkeeping split either side of the caller's
// own `vfs` publish, plus rename/replace (a materialize-shaped transaction
// over a different destination).
// ---------------------------------------------------------------------

pub(crate) fn materialize_prepare(
    conn: &mut Connection,
    session_id: SessionId,
    doc_id: DocId,
    target: MaterializeTarget,
) -> Result<OpOutcome, Error> {
    let prep = crate::materialize::prepare_materialize(
        conn,
        crate::materialize::DocSession { doc_id, session_id },
        target,
    )?;
    Ok(OpOutcome::MaterializePrep(Box::new(prep)))
}

pub(crate) fn materialize_record(
    conn: &mut Connection,
    session_id: SessionId,
    doc_id: DocId,
    resolved_path: PathBuf,
    seq: i64,
    now: SystemTime,
    outcome: MaterializeOutcome,
) -> Result<OpOutcome, Error> {
    let result = crate::materialize::record_materialize_outcome(
        conn,
        crate::materialize::DocSession { doc_id, session_id },
        &resolved_path,
        seq,
        now,
        outcome,
    )?;
    Ok(OpOutcome::Materialize(Box::new(result)))
}

pub(crate) fn rename_file(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: SessionId,
    doc_id: DocId,
    from: PathBuf,
    to: PathBuf,
    now: SystemTime,
) -> Result<OpOutcome, Error> {
    let outcome = crate::rename_bind::rename_bind(
        conn,
        vfs,
        crate::materialize::DocSession { doc_id, session_id },
        &from,
        &to,
        now,
    )?;
    Ok(OpOutcome::Rename(Box::new(outcome)))
}

pub(crate) struct RenameReplaceArgs {
    pub(crate) session_id: SessionId,
    pub(crate) doc_id: DocId,
    pub(crate) from: PathBuf,
    pub(crate) to: PathBuf,
    pub(crate) seen: Stat,
    pub(crate) now: SystemTime,
}

pub(crate) fn rename_replace(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    args: RenameReplaceArgs,
) -> Result<OpOutcome, Error> {
    let outcome = crate::rename_replace::rename_replace(
        conn,
        vfs,
        crate::materialize::DocSession {
            doc_id: args.doc_id,
            session_id: args.session_id,
        },
        &args.from,
        &args.to,
        args.seen,
        args.now,
    )?;
    Ok(OpOutcome::Rename(Box::new(outcome)))
}

// ---------------------------------------------------------------------
// Document-lifecycle ops: bind/load, adoption, scratch bookkeeping,
// search history, shutdown.
// ---------------------------------------------------------------------

pub(crate) struct LoadArgs {
    pub(crate) session_id: SessionId,
    pub(crate) liveness_check: LivenessCheckFn,
    pub(crate) path: PathBuf,
    pub(crate) now: SystemTime,
    pub(crate) source: LoadSource,
}

pub(crate) fn load(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    undo_state: &mut HashMap<DocId, DocUndoState>,
    args: LoadArgs,
) -> Result<OpOutcome, Error> {
    let result = match args.source {
        LoadSource::Fresh => crate::load::load(
            conn,
            vfs,
            args.session_id,
            args.liveness_check.as_ref(),
            &args.path,
            args.now,
        )?,
        LoadSource::Taken(sighting) => {
            let read = crate::bracket::BracketedRead {
                data: sighting.bytes,
                stat: crate::bracket::stat_facts_from(sighting.sighted.stat()),
                confirmed: sighting.sighted.is_confirmed(),
            };
            crate::load::load_from_read(
                conn,
                vfs,
                args.session_id,
                args.liveness_check.as_ref(),
                &args.path,
                read,
                args.now,
            )?
        }
    };
    // A fresh binding — this document's LOCAL undo-journal position
    // `0` (no local pushes yet this binding) durably predates
    // `bridge_seq` if this load journaled a cross-session
    // inheritance bridge edit, else it predates whatever this
    // session already found at `doc_id` (0 for a genuinely fresh
    // document). Replaces, never merges with, any stale entry a
    // PRIOR binding of this same `doc_id` left behind (a close then
    // reopen within one process resets local position numbering
    // right along with it).
    undo_state.insert(
        result.doc_id,
        DocUndoState {
            base_seq: result.bridge_seq.unwrap_or(crate::ids::Seq(0)),
            local_seq: Vec::new(),
        },
    );
    Ok(OpOutcome::Load(Box::new(result)))
}

pub(crate) fn resolve_adopt(
    conn: &mut Connection,
    session_id: SessionId,
    doc_id: DocId,
    obs: ObsId,
    edit_seq: Option<i64>,
    now: SystemTime,
) -> Result<OpOutcome, Error> {
    let observation = crate::adopt::resolve_adopt(conn, session_id, doc_id, obs, edit_seq, now)?;
    Ok(OpOutcome::Observation(Box::new(observation)))
}

pub(crate) fn resolve_abandon(
    conn: &mut Connection,
    session_id: SessionId,
    doc_id: DocId,
) -> Result<OpOutcome, Error> {
    crate::adopt::resolve_abandon(conn, session_id, doc_id)?;
    Ok(OpOutcome::None)
}

pub(crate) fn create_scratch(
    conn: &mut Connection,
    undo_state: &mut HashMap<DocId, DocUndoState>,
    session_id: SessionId,
    now: SystemTime,
    intended_path: Option<String>,
) -> Result<OpOutcome, Error> {
    let id = crate::scratch::create_scratch_with_intent(
        conn,
        session_id,
        now,
        intended_path.as_deref(),
    )?;
    // A brand-new row, never bound before — local position `0`
    // starts at durable seq `0`, same as `Load`'s doc comment.
    undo_state.insert(id, DocUndoState::default());
    Ok(OpOutcome::ScratchDocId(id))
}

pub(crate) fn gc_empty_scratch(
    conn: &mut Connection,
    keep_id: i64,
    liveness_check: &LivenessCheckFn,
) -> Result<OpOutcome, Error> {
    crate::scratch::gc_empty_scratch(conn, keep_id, liveness_check.as_ref())?;
    Ok(OpOutcome::None)
}

pub(crate) fn recoverable_scratch(
    conn: &mut Connection,
    exclude_id: i64,
) -> Result<OpOutcome, Error> {
    let ids = crate::scratch::recoverable_scratch(conn, exclude_id)?;
    Ok(OpOutcome::Ids(ids))
}

pub(crate) fn find_named_scratch(
    conn: &mut Connection,
    intended_path: &str,
) -> Result<OpOutcome, Error> {
    let ids = crate::scratch::find_named_scratch(conn, intended_path)?;
    Ok(OpOutcome::Ids(ids))
}

pub(crate) fn reconstruct_scratch(
    conn: &mut Connection,
    liveness_check: &LivenessCheckFn,
    doc_id: DocId,
) -> Result<OpOutcome, Error> {
    let content = crate::scratch::reconstruct_scratch(conn, liveness_check.as_ref(), doc_id)?;
    Ok(OpOutcome::Reconstructed(content))
}

pub(crate) fn touch_search_query(
    conn: &mut Connection,
    query: &str,
    now: SystemTime,
) -> Result<OpOutcome, Error> {
    retry::with_retry(conn, |tx| crate::search_history::touch(tx, query, now))?;
    Ok(OpOutcome::None)
}

pub(crate) fn touch_command_name(
    conn: &mut Connection,
    name: &str,
    now: SystemTime,
) -> Result<OpOutcome, Error> {
    retry::with_retry(conn, |tx| crate::command_history::touch(tx, name, now))?;
    Ok(OpOutcome::None)
}

pub(crate) fn shutdown(
    conn: &mut Connection,
    session_id: SessionId,
    liveness_check: &LivenessCheckFn,
) -> Result<OpOutcome, Error> {
    crate::writer_lifecycle::run_shutdown_maintenance(conn, session_id, liveness_check.as_ref());
    Ok(OpOutcome::None)
}
