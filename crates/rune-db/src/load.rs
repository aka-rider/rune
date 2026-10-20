//! `Load` — reads a path fresh from disk, resolves document identity,
//! records the sighting, and returns everything the caller needs to decide
//! how to display it. Every step below
//! either does `vfs` I/O with no transaction open, or opens its own short
//! `retry::with_retry` transaction (plan binding rule, invariant I1).
//!
//! The Adoption Contract (`observation.rs` module doc) governs `saved_obs`
//! here: first-ever load anchors a recovery snapshot and adopts it as-is;
//! a hash-equal reload heal-adopts (crash-between-swap-and-ack recovery); a
//! divergent reload records a bare, uncorrelated sighting and leaves
//! `saved_obs` untouched.

use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, Transaction, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::adopt;
use crate::bracket;
use crate::confirmation::Confirmation;
use crate::document::{self, DocRef};
use crate::history::{HistoryLoss, Lineage, read_lineage};
use crate::ids::{DocId, ObsId, SessionId};
use crate::inherit::find_inheritable_draft;
use crate::load_anchor::{LoadContext, anchor_first_load, reanchor_clean_reload_tx};
use crate::obs_origin::ObsOrigin;
use crate::observation;
use crate::retry;
use crate::snapshot::Recovered;
use crate::succession::{Adoption, adopt_dead_lineage};
use crate::sync::SyncState;

/// The outcome of [`load`]: the raw disk bytes, the journal-reconstructed
/// content (identical to `disk_content` when the document has no history),
/// and the [`SyncState`] that follows from recording this sighting.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadResult {
    pub doc_id: DocId,
    pub renamed_from: Option<String>,
    pub disk_content: String,
    pub recovered: Recovered,
    pub has_history: bool,
    pub sync: SyncState,
    /// Hard-link count observed at load (0 if stat failed or the platform
    /// doesn't expose it) — `nlink > 1` means saving through this path
    /// forks the document from its other names on disk.
    pub nlink: i64,
    /// This session's CAS baseline for `doc_id` (`session_documents.
    /// saved_obs`) once `load` returns — the `expect` `Store::materialize`
    /// needs for this session's very first save. `None` only if
    /// adoption somehow never happened for this session/doc pair (should
    /// not occur — every branch of `load` above either adopts or leaves a
    /// PRIOR adoption of this session's own in place — kept `Option` rather
    /// than assumed, matching this crate's "Options for absent facts" rule
    /// rather than a caller-visible panic risk).
    pub saved_obs: Option<ObsId>,
    pub history: Lineage,
    pub history_losses: Vec<HistoryLoss>,
    /// A dead session's still-`active` merge whose recorded working form
    /// byte-matches this load's own journal reconstruction — the caller may
    /// re-enter it; the row stays `active` until a hydrating ack actually
    /// does. `None` when there is no such merge (or its recorded form no
    /// longer matches, in which case the row was flipped to `abandoned`).
    pub resumable_merge: Option<crate::merge_state::ResumableMerge>,
}

/// Reports whether `doc_id` has any events or snapshots RECORDED BY
/// `session_id`.
pub(crate) fn has_history(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
) -> Result<bool, Error> {
    tx.query_row(
        "SELECT EXISTS( \
            SELECT 1 FROM events WHERE doc_id=?1 AND session_id=?2 \
            UNION ALL \
            SELECT 1 FROM snapshots WHERE doc_id=?1 AND session_id=?2 \
         )",
        params![doc_id, session_id],
        |r| r.get(0),
    )
    .map_err(Error::from)
}

/// Reads `path` fresh from disk, resolves its document identity, records
/// the sighting, and returns everything the caller needs to decide how to
/// display it. `liveness_check` is
/// threaded in per-call (this `Store`'s injected liveness function) rather
/// than read from shared state — `OpKind::Load` carries it.
pub(crate) fn load(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: SessionId,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    path: &Path,
    now: SystemTime,
) -> Result<LoadResult, Error> {
    let resolved = vfs.resolve(path).map_err(Error::Io)?;
    // Bracketed (stat, read, re-stat) — `document::open_path` below requires
    // the read to have already happened, and every disk-sourced fact this
    // load records (the anchor, the heal-adopt, the bare divergent sighting)
    // must come from a read a racer caught mid-external-rewrite cannot
    // masquerade as stable.
    let read = bracket::bracketed_read(vfs, &resolved).map_err(Error::Io)?;
    load_from_read(conn, vfs, session_id, liveness_check, &resolved, read, now)
}

/// The rest of [`load`], starting from an ALREADY-taken [`bracket::BracketedRead`]
/// rather than reading `path` fresh from disk itself — the chokepoint a
/// caller that already sighted `path` through its own single read (`rune_vfs::
/// get`, adapted into the same bracket vocabulary) funnels through instead of
/// [`load`] taking a second, independent read of the same path. `path` must
/// already be resolved, exactly as [`load`] resolves it before delegating
/// here. An unconfirmed `read` is adopted exactly as `load`'s own bracket
/// does when its retries exhaust — `confirm_against_history` already handles
/// `confirmed: false` correctly.
pub fn load_from_read(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: SessionId,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    path: &Path,
    read: bracket::BracketedRead,
    now: SystemTime,
) -> Result<LoadResult, Error> {
    let data = read.data;
    let stat = read.stat;

    let doc_ref: DocRef = document::open_path(conn, vfs, path, now)?;
    let doc_id = doc_ref.id;

    // Hashing and blob storage operate on the raw disk bytes UNCONDITIONALLY
    // — a load must still durably record what's actually on disk even if it
    // turns out not to be valid UTF-8 (blob.rs's module doc: disk-sourced
    // content is never gated on decode success). Only the `content: String`
    // this function ultimately returns for the edit buffer requires the
    // file to be genuinely valid text — that check happens below, AFTER
    // this sighting is already durably recorded.
    let hash = retry::with_retry(conn, |tx| crate::blob::put_blob(tx, &data))?;

    // Folds the suspicious-shrink gate against `doc_id`'s own confirmed
    // history into the bracket's own verdict — the SAME rule
    // `probe::probe`'s fresh reads apply, now shared via
    // `confirm_against_history` rather than reimplemented here.
    let disk_confirmed = retry::with_retry(conn, |tx| {
        bracket::confirm_against_history(tx, doc_id, read.confirmed, data.len(), &hash)
    })?;

    // BEFORE recording anything below — must reflect GENUINE prior history,
    // not history this very call is about to create. A dead session's
    // replayable lineage is adopted first, so everything after sees it as
    // this session's own history.
    let had_history = retry::with_retry(conn, |tx| has_history(tx, session_id, doc_id))?;
    let adoption = if had_history {
        Adoption::NothingToAdopt
    } else {
        adopt_dead_lineage(conn, liveness_check, session_id, doc_id, now)?
    };
    let has_hist = had_history || matches!(adoption, Adoption::Adopted);
    let declined_loss = match adoption {
        Adoption::Lost(loss) => Some(loss),
        Adoption::Adopted | Adoption::NothingToAdopt => None,
    };
    let position_repaired = has_hist
        && retry::with_retry(conn, |tx| {
            crate::snapshot::repair_stranded_position(tx, session_id, doc_id)
        })?;

    // The journal position AT LOAD TIME.
    let load_seq = retry::with_retry(conn, |tx| {
        crate::journal::current_seq(tx, session_id, doc_id)
    })?;

    // Content re-enters the String-typed edit buffer HERE: this is
    // session-editable content, which must be valid UTF-8 (rune-core's
    // `Buffer` has no other representation) — a load of genuinely non-text
    // content is a real, surfaced error, never silently mangled or dropped.
    let content = String::from_utf8(data)
        .map_err(|e| Error::Invalid(format!("load {}: non-utf8 content: {e}", path.display())))?;

    let (recovered, redo_tail_dropped) = if !has_hist {
        // Cross-session crash recovery (v10, B2/R2) for a dead session whose
        // lineage adoption declined — MUST run before this session writes
        // anything of its own for doc_id, else it would immediately find
        // itself as "the most recent session". First-ever load: the sighting IS the adoption — the recovery anchor MUST
        // commit before the adoption (which asserts "the journal
        // reconstruction at load_seq equals this blob"). The anchor and the
        // adoption use disk content/hash for `Inherited::Disk`/`Bridged`;
        // `Inherited::Diverged` instead re-anchors on the dead session's own
        // baseline (`load_anchor`'s own doc comment) so the newer disk
        // content is never silently discarded.
        let inherited = find_inheritable_draft(conn, liveness_check, doc_id, &hash)?;
        let ctx = LoadContext {
            session_id,
            doc_id,
            load_seq,
            disk_hash: &hash,
            disk_confirmed,
            live_stat: &stat,
            now,
        };
        (anchor_first_load(conn, &ctx, &content, &inherited)?, false)
    } else {
        let recovered = retry::with_retry(conn, |tx| {
            crate::snapshot::recover_document(tx, session_id, doc_id)
        })?;
        let recovered_hash = observation::hash_bytes(recovered.content.as_bytes());
        if hash == recovered_hash {
            // Reload, hash-equality: heal-adopt only when there is something to
            // heal (an ordinary clean tab switch also lands here).
            let cur = retry::with_retry(conn, |tx| {
                observation::saved_obs_for(tx, session_id, doc_id)
            })?;
            let needs_heal = cur.as_ref().is_none_or(|c| c.blob_hash.as_str() != hash);
            if needs_heal {
                adopt::record_adoption(
                    conn,
                    doc_id,
                    session_id,
                    observation::ObservationMeta {
                        blob_hash: &hash,
                        seq: Some(load_seq.0),
                        origin: ObsOrigin::Resolve,
                        confirmed: Confirmation::from_bracket(disk_confirmed),
                    },
                    &stat,
                    now,
                    None,
                )?;
            }
            (recovered, false)
        } else {
            // Reload, hashes differ: this session's own last-adopted baseline
            // decides whether disk moved out from under an untouched buffer
            // (adopt it cleanly) or out from under real unsaved edits (leave
            // them alone and just record the sighting).
            let baseline = retry::with_retry(conn, |tx| {
                observation::saved_obs_for(tx, session_id, doc_id)
            })?;
            let no_unsaved_edits = baseline
                .as_ref()
                .is_some_and(|b| b.blob_hash.as_str() == recovered_hash);
            if no_unsaved_edits {
                let ctx = LoadContext {
                    session_id,
                    doc_id,
                    load_seq,
                    disk_hash: &hash,
                    disk_confirmed,
                    live_stat: &stat,
                    now,
                };
                let reanchored =
                    reanchor_clean_reload_tx(conn, &ctx, &recovered.content, &content)?;
                (reanchored.recovered, reanchored.redo_dropped)
            } else {
                // A bare, uncorrelated sighting — saved_obs stays exactly
                // where it was, and the unsaved edits stay in the buffer.
                let at = crate::session::format_rfc3339_nanos(now);
                retry::with_retry(conn, |tx| {
                    observation::record_observation(
                        tx,
                        doc_id,
                        session_id,
                        observation::ObservationMeta {
                            blob_hash: &hash,
                            seq: None,
                            origin: ObsOrigin::Load,
                            confirmed: Confirmation::from_bracket(disk_confirmed),
                        },
                        &stat,
                        &at,
                    )
                })?;
                (recovered, false)
            }
        }
    };

    let (history, lineage_loss) = retry::with_retry(conn, |tx| {
        read_lineage(tx, session_id, doc_id, &recovered.content, now)
    })?;
    let history_losses: Vec<HistoryLoss> = [
        declined_loss,
        lineage_loss,
        position_repaired.then_some(HistoryLoss::PositionRepaired),
        redo_tail_dropped.then_some(HistoryLoss::RedoTailDropped),
    ]
    .into_iter()
    .flatten()
    .collect();

    let recovered_hash = observation::hash_bytes(recovered.content.as_bytes());
    let resumable_merge = retry::with_retry(conn, |tx| {
        crate::merge_state::resume_candidate(tx, liveness_check, doc_id, &recovered_hash)
    })?;

    let sync_state = retry::with_retry(conn, |tx| crate::sync::sync(tx, session_id, doc_id))?;
    let saved_obs = retry::with_retry(conn, |tx| {
        observation::saved_obs_for(tx, session_id, doc_id)
    })?
    .map(|o| o.id);

    Ok(LoadResult {
        doc_id,
        renamed_from: doc_ref.renamed_from,
        disk_content: content,
        recovered,
        has_history: has_hist,
        sync: sync_state,
        nlink: stat.nlink.unwrap_or(0),
        saved_obs,
        history,
        history_losses,
        resumable_merge,
    })
}

#[cfg(test)]
#[path = "load_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "load_reopen_tests.rs"]
mod reopen_tests;
