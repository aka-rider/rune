//! `Probe` — refreshes a document's disk fact.
//! Unconditionally reads and hashes the live
//! target, records the sighting as `origin='probe'`, and classifies the
//! result — never moving `saved_obs` on a bare divergence (a probe is
//! passive observation, not consent to overwrite). The ONE exception is
//! auto-adopt: when the fresh read hash-equals the journal-head
//! reconstruction (`SyncKind::Clean`), this is unambiguously our own prior
//! write (most likely a `Materialize` whose ack tx never committed before a
//! crash) and is promoted to a real adoption via `adopt::adopt_equal`.
//!
//! `SyncKind::DiskAhead` (clean buffer, disk moved) deliberately gets NO such
//! auto-adopt, even though the buffer has nothing to lose: `saved_obs` is
//! also `materialize`'s CAS baseline, and this module reads bytes but never
//! writes the journal. Moving `saved_obs` to theirs here, with the journal
//! reconstruction left unchanged at the OLD content, would let a plain
//! unconditional save that follows CAS-succeed against the new baseline and
//! overwrite disk's legitimate newer content with the buffer's stale bytes —
//! trading a cosmetic stale-baseline annoyance for the exact silent-overwrite
//! this crate exists to prevent. The safe fast-forward for this shape
//! already exists one layer up: `merge_prep`/the resolver's zero-conflict
//! path installs theirs' bytes into the buffer AND advances `saved_obs`
//! together, atomically, whenever the caller invites it.
//!
//! Runs as a writer-FIFO op (`OpKind::Probe`): every step below either does
//! `vfs` I/O with no transaction open, or opens its own short
//! `retry::with_retry` transaction — never both at once (plan binding rule,
//! invariant I1).

use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::adopt;
use crate::bracket;
use crate::confirmation::Confirmation;
use crate::ids::{DocId, SessionId};
use crate::obs_origin::ObsOrigin;
use crate::observation;
use crate::retry;
use crate::sync::{self, SyncKind, SyncState, Version};

/// Refreshes `doc_id`'s disk fact and returns the resulting [`SyncState`].
/// A `documents.path` of `""` (untitled/scratch/chat) has nothing on disk
/// to probe and degrades to a pure [`sync::sync`]. A target that has gone
/// missing surfaces [`Error::NotFound`] — the workspace layer's
/// deleted-guard trigger (WP5).
pub(crate) fn probe(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: SessionId,
    doc_id: DocId,
    now: SystemTime,
) -> Result<SyncState, Error> {
    let path: String = retry::with_retry(conn, |tx| {
        tx.query_row(
            "SELECT path FROM documents WHERE id=?1",
            params![doc_id],
            |r| r.get(0),
        )
        .map_err(Error::from)
    })?;

    if path.is_empty() {
        return retry::with_retry(conn, |tx| sync::sync(tx, session_id, doc_id));
    }

    let path = PathBuf::from(path);
    let resolved = vfs.resolve(&path).map_err(Error::Io)?;

    if let Err(e) = vfs.stat(&resolved) {
        if e.kind() == io::ErrorKind::NotFound {
            return Err(Error::NotFound(format!(
                "probe doc {doc_id}: {}",
                path.display()
            )));
        }
        return Err(Error::Io(e));
    }

    // Stat short-circuit (plan Gotchas [R2]): a probe's default action reads
    // the whole file and inserts a fresh observation on EVERY call, which
    // would grow the store unboundedly if enqueued on every tab switch. When
    // the live stat identity/size/mtime already match the newest recorded
    // observation AND that observation is CONFIRMED, nothing on disk has
    // moved since that sighting was taken — classify against it directly,
    // with no re-read and no new row. An unconfirmed newest observation never
    // short-circuits: it decides nothing, including "nothing changed".
    let stat = observation::stat_identity(vfs, &resolved);
    let existing = retry::with_retry(conn, |tx| observation::newest_observation(tx, doc_id))?;
    let unchanged = existing.filter(|o| {
        o.confirmed == Confirmation::Confirmed
            && o.size == stat.size
            && o.mtime == stat.mtime
            && o.inode == stat.inode
            && o.device == stat.device
    });

    let theirs_obs = match unchanged {
        Some(obs) => obs,
        None => {
            // Recorded as a raw-bytes blob regardless of UTF-8 validity — a
            // probe is a passive observation of whatever is actually on disk
            // (blob.rs module doc); it must never hard-fail just because the
            // file isn't valid text. `observe_disk` brackets the read
            // (stat-read-stat) and folds in the suspicious-shrink gate before
            // recording it — a probe can never trust a read caught
            // mid-external-rewrite as a stable fact.
            bracket::observe_disk(
                conn,
                vfs,
                session_id,
                doc_id,
                &resolved,
                bracket::ObserveDiskMeta {
                    seq: None,
                    origin: ObsOrigin::Probe,
                },
                now,
            )?
        }
    };

    let theirs = Some(Version {
        hash: theirs_obs.blob_hash.clone(),
        obs: Some(theirs_obs.id),
    });
    let state = retry::with_retry(conn, |tx| {
        sync::sync_with_theirs(tx, session_id, doc_id, theirs.clone())
    })?;

    if state.kind == SyncKind::Clean {
        // Auto-adopt only when there is something to heal: stacking a fresh
        // 'resolve' adoption on every clean probe tick would grow
        // observations/parent_a unboundedly.
        let should_adopt = retry::with_retry(conn, |tx| {
            let cur = observation::saved_obs_for(tx, session_id, doc_id)?;
            Ok::<bool, Error>(match cur {
                None => true,
                Some(c) => c.blob_hash != theirs_obs.blob_hash,
            })
        })?;
        if should_adopt {
            let pos = retry::with_retry(conn, |tx| {
                crate::journal::current_seq(tx, session_id, doc_id)
            })?;
            let _ = adopt::adopt_equal(conn, session_id, doc_id, theirs_obs.id, pos.0, now)?;
        }
    }

    Ok(state)
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
