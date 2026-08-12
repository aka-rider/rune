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
use crate::inherit::find_inheritable_draft;
use crate::load_anchor::{LoadContext, anchor_first_load};
use crate::obs_origin::ObsOrigin;
use crate::observation::{self, ObsId};
use crate::retry;
use crate::sync::SyncState;

/// The outcome of [`load`]: the raw disk bytes, the journal-reconstructed
/// content (identical to `disk_content` when the document has no history),
/// and the [`SyncState`] that follows from recording this sighting.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadResult {
    pub doc_id: i64,
    pub renamed_from: Option<String>,
    pub disk_content: String,
    pub recovered: String,
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
    /// The durable journal seq THIS session's own cross-session-inheritance
    /// bridge edit (`find_inheritable_draft`'s synthetic replace-all,
    /// journaled under `session_id`) committed to, when `load` performed
    /// one — `None` when it didn't (no inheritance, or `has_history` was
    /// already true). This is this session's own durable journal HEAD
    /// immediately after `load` returns: a fresh session has journaled
    /// nothing else of its own before or during `load`, so a caller seeding
    /// its local "last acked durable seq" tracking (`AppDb::last_known_seq`)
    /// MUST start from this, not an assumed `0` — an `undo`/`redo`
    /// committing `move_undo_pos` before its first ordinary `AppendEdit` ack
    /// lands would otherwise silently regress this session's own
    /// `current_seq` past a bridge edit that already advanced it.
    pub bridge_seq: Option<i64>,
    /// A dead session's still-`active` merge whose recorded working form
    /// byte-matches this load's own journal reconstruction — the caller may
    /// re-enter it; the row stays `active` until a hydrating ack actually
    /// does. `None` when there is no such merge (or its recorded form no
    /// longer matches, in which case the row was flipped to `abandoned`).
    pub resumable_merge: Option<crate::merge_state::ResumableMerge>,
}

/// Reports whether `doc_id` has any events or snapshots RECORDED BY
/// `session_id`.
pub fn has_history(tx: &Transaction<'_>, session_id: i64, doc_id: i64) -> Result<bool, Error> {
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
pub fn load(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: i64,
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
    session_id: i64,
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

    // The journal position AT LOAD TIME.
    let load_seq = retry::with_retry(conn, |tx| {
        crate::journal::current_seq(tx, session_id, doc_id)
    })?;

    // Folds the suspicious-shrink gate against `doc_id`'s own confirmed
    // history into the bracket's own verdict — the SAME rule
    // `probe::probe`'s fresh reads apply, now shared via
    // `confirm_against_history` rather than reimplemented here.
    let disk_confirmed = retry::with_retry(conn, |tx| {
        bracket::confirm_against_history(tx, doc_id, read.confirmed, data.len(), &hash)
    })?;

    // BEFORE recording anything below — must reflect GENUINE prior history,
    // not history this very call is about to create.
    let has_hist = retry::with_retry(conn, |tx| has_history(tx, session_id, doc_id))?;

    // Content re-enters the String-typed edit buffer HERE: this is
    // session-editable content, which must be valid UTF-8 (rune-core's
    // `Buffer` has no other representation) — a load of genuinely non-text
    // content is a real, surfaced error, never silently mangled or dropped.
    let content = String::from_utf8(data)
        .map_err(|e| Error::Invalid(format!("load {}: non-utf8 content: {e}", path.display())))?;

    let mut recovered = content.clone();
    let mut bridge_seq = None;
    if has_hist {
        recovered = retry::with_retry(conn, |tx| {
            crate::snapshot::recover_document(tx, session_id, doc_id)
        })?;
    }

    if !has_hist {
        // Cross-session crash recovery (v10, B2/R2) — MUST run before this
        // session writes anything of its own for doc_id, else it would
        // immediately find itself as "the most recent session". First-ever
        // load: the sighting IS the adoption — the recovery anchor MUST
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
        let outcome = anchor_first_load(conn, &ctx, &content, inherited)?;
        recovered = outcome.recovered;
        bridge_seq = outcome.bridge_seq;
    } else if hash == observation::hash_bytes(recovered.as_bytes()) {
        // Reload, hash-equality: heal-adopt only when there is something to
        // heal (an ordinary clean tab switch also lands here).
        let cur = retry::with_retry(conn, |tx| {
            observation::saved_obs_for(tx, session_id, doc_id)
        })?;
        let needs_heal = match &cur {
            None => true,
            Some(c) => c.blob_hash != hash,
        };
        if needs_heal {
            adopt::record_adoption(
                conn,
                doc_id,
                session_id,
                observation::ObservationMeta {
                    blob_hash: &hash,
                    seq: Some(load_seq),
                    origin: ObsOrigin::Resolve,
                    confirmed: Confirmation::from_bracket(disk_confirmed),
                },
                &stat,
                now,
                None,
            )?;
        }
    } else {
        // Reload, hashes differ: a bare, uncorrelated sighting — saved_obs
        // stays exactly where it was.
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
    }

    let recovered_hash = observation::hash_bytes(recovered.as_bytes());
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
        bridge_seq,
        resumable_merge,
    })
}

#[cfg(test)]
#[path = "load_tests.rs"]
mod tests;
