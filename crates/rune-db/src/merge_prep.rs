//! `MergePrep` — plan WP3.S1's merge-entry fresh-state read: runs
//! [`probe::probe`] (the same disk fact refresh a tab-switch probe does)
//! and returns the ancestor/theirs BYTES from the same op, not just their
//! hashes. `sync.rs`'s own `SyncState` carries hashes only (plan Gotchas
//! `[B2]`: "No existing non-blocking path returns blob bytes to the TUI")
//! — merge entry needs the actual content to feed `rune_merge::merge_hunks`,
//! and re-deriving it from a LATER, separately-timed read would reopen
//! exactly the race `[B2]` exists to close. `probe::probe` itself runs as
//! several short, separately-committed transactions (its own retry loop),
//! not one — the actual guarantee this function relies on is the
//! single-writer FIFO every op on this connection already runs under: no
//! OTHER op can interleave between the probe and the blob reads below, so
//! the observation `theirs`/`ancestor` are read against is still the
//! newest one by the time this op hands its result back.

use std::time::SystemTime;

use rusqlite::Connection;

use rune_vfs::Vfs;

use crate::Error;
use crate::blob;
use crate::confirmation::Confirmation;
use crate::ids::{DocId, ObsId, SessionId};
use crate::observation;
use crate::probe;
use crate::retry;
use crate::sync::SyncState;

/// The bound on how many times [`merge_prep`] re-probes while the theirs
/// observation it would serve stays unconfirmed — bounded, no wall-clock
/// pacing: each retry is an immediate fresh bracketed read, never a sleep.
const MERGE_PREP_MAX_ATTEMPTS: u32 = 3;

/// Which rung of the ancestor ladder (module doc) produced
/// [`MergePrepOutcome::Ready`]'s `ancestor` — so the caller can present the
/// truth honestly instead of treating every `None` the same way
/// `landing.rs`'s `unwrap_or("")` used to (silently substituting an empty
/// ancestor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AncestorRung {
    /// Walking the observations' own parent-edge lineage found a common
    /// ancestor between this session's CAS baseline and theirs — possibly
    /// fresher or more precise than the session-scoped derivation below,
    /// since it can see edges recorded by adoptions this session's own
    /// journal position never correlates to.
    Lineage,
    /// The lineage walk found nothing (no baseline, no theirs observation,
    /// or no intersecting edge) — today's fallback: `sync_with_theirs`'s
    /// own session-scoped, seq-correlated `ancestor_at` derivation.
    SessionScoped,
}

/// `MergePrep`'s result: the freshly classified [`SyncState`] plus the
/// outcome the fresh read reached.
#[derive(Clone, Debug, PartialEq)]
pub struct MergePrepResult {
    pub sync: SyncState,
    pub outcome: MergePrepOutcome,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MergePrepOutcome {
    Unstable,
    Ready {
        ancestor: Option<(AncestorRung, Vec<u8>)>,
        theirs: Option<(ObsId, Vec<u8>)>,
    },
}

/// Whether `sync`'s theirs fact, if it names an observation at all, is
/// CONFIRMED — `true` when there is nothing to confirm (`Clean`/
/// `BufferAhead` carry no theirs a merge would ever serve).
fn theirs_confirmed(conn: &mut Connection, sync: &SyncState) -> Result<bool, Error> {
    let Some(obs_id) = sync.theirs.as_ref().and_then(|v| v.obs) else {
        return Ok(true);
    };
    let obs = retry::with_retry(conn, |tx| observation::get_observation(tx, obs_id))?;
    Ok(obs.confirmed == Confirmation::Confirmed)
}

/// Runs the fresh-state read for `doc_id`, collapsing what would otherwise
/// be a probe and a separate blob re-read into one op, so the TUI's
/// `update` never has to correlate two async round trips for one merge
/// attempt. Re-probes (bounded by [`MERGE_PREP_MAX_ATTEMPTS`]) while the
/// theirs observation stays unconfirmed — a merge must never serve content
/// a racer may have caught mid-external-rewrite as Theirs.
pub fn merge_prep(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: SessionId,
    doc_id: DocId,
    now: SystemTime,
) -> Result<MergePrepResult, Error> {
    let mut sync = probe::probe(conn, vfs, session_id, doc_id, now)?;
    let mut confirmed = theirs_confirmed(conn, &sync)?;
    let mut attempts = 1;
    while !confirmed && attempts < MERGE_PREP_MAX_ATTEMPTS {
        sync = probe::probe(conn, vfs, session_id, doc_id, now)?;
        confirmed = theirs_confirmed(conn, &sync)?;
        attempts += 1;
    }
    if !confirmed {
        return Ok(MergePrepResult {
            sync,
            outcome: MergePrepOutcome::Unstable,
        });
    }

    // `theirs` stays `None` whenever `sync.theirs` is `None` — unreachable
    // in practice with `kind` in `DiskAhead`/`Diverged` (`classify_sync`'s
    // `None` branch only ever yields `Clean`/`BufferAhead`), which is
    // exactly what the caller's authoritative gate checks before trusting
    // it.
    let theirs_obs = sync.theirs.as_ref().and_then(|v| v.obs);
    let theirs = match &sync.theirs {
        Some(version) => {
            let bytes = blob::get_blob(conn, version.hash.as_str())?;
            theirs_obs.map(|obs| (obs, bytes))
        }
        None => None,
    };
    let ancestor = resolve_ancestor(conn, session_id, doc_id, &sync, theirs_obs)?;

    Ok(MergePrepResult {
        sync,
        outcome: MergePrepOutcome::Ready { ancestor, theirs },
    })
}

/// The ancestor ladder: (i) walk the parent-edge lineage between this
/// session's CAS baseline and `theirs_obs` for a common ancestor — sees
/// edges recorded by ANY adoption or confirmed sighting, not only this
/// session's own seq-correlated agreements; (ii) failing that, fall back to
/// `sync`'s own session-scoped `ancestor_at` derivation (today's rule); (iii)
/// failing THAT, report absence explicitly rather than ever substituting an
/// empty ancestor.
fn resolve_ancestor(
    conn: &mut Connection,
    session_id: SessionId,
    doc_id: DocId,
    sync: &SyncState,
    theirs_obs: Option<ObsId>,
) -> Result<Option<(AncestorRung, Vec<u8>)>, Error> {
    let baseline = retry::with_retry(conn, |tx| {
        observation::saved_obs_for(tx, session_id, doc_id)
    })?;
    if let (Some(baseline), Some(theirs_id)) = (baseline, theirs_obs) {
        let lca = retry::with_retry(conn, |tx| {
            crate::lineage::common_ancestor(tx, baseline.id, theirs_id)
        })?;
        if let Some(node) = lca {
            let bytes = blob::get_blob(conn, node.blob_hash.as_str())?;
            return Ok(Some((AncestorRung::Lineage, bytes)));
        }
    }
    match &sync.ancestor {
        Some(version) => {
            let bytes = blob::get_blob(conn, version.hash.as_str())?;
            Ok(Some((AncestorRung::SessionScoped, bytes)))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
#[path = "merge_prep_tests.rs"]
mod tests;
