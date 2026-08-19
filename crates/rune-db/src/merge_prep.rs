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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::obs_origin::ObsOrigin;
    use crate::test_support::open;
    use rune_vfs::{Mem, VfsTestExt};
    use std::path::Path;

    fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
        let temp = vfs.write_durable(path, bytes).expect("write_durable");
        vfs.rename_excl(&temp, path).expect("publish");
    }

    /// Task WP-C(3): when this session's own CAS baseline was adopted with
    /// no correlated seq (so `ancestor_at`'s session-scoped derivation can
    /// never find it — `sync.ancestor` stays `None`), but that SAME
    /// baseline is still reachable from the fresh `theirs` sighting via the
    /// observations' own parent-edge lineage, the ladder's rung (i) must
    /// still surface it rather than reporting absence.
    #[test]
    fn merge_prep_ancestor_ladder_prefers_lineage_over_an_absent_session_scoped_ancestor() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"baseline content");

        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = DocId(conn.last_insert_rowid());

        let stat = observation::StatFacts {
            size: Some(1),
            mtime: Some("t".to_string()),
            ..Default::default()
        };
        let hash_baseline = {
            let tx = conn.transaction().expect("tx");
            let h = crate::blob::put_blob(&tx, b"baseline content").expect("seed blob");
            tx.commit().expect("commit");
            h
        };
        // Adopted with NO correlated seq: `ancestor_at` requires `seq IS
        // NOT NULL`, so this baseline can never surface through the
        // session-scoped rung no matter the journal position.
        crate::adopt::record_adoption(
            &mut conn,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash_baseline,
                seq: None,
                origin: ObsOrigin::Resolve,
                confirmed: Confirmation::Confirmed,
            },
            &stat,
            SystemTime::now(),
            None,
        )
        .expect("seed baseline adoption");

        let temp = vfs
            .write_durable(path, b"theirs content")
            .expect("write_durable");
        vfs.exchange(&temp, path).expect("exchange");
        let result =
            merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");

        assert_eq!(result.sync.kind, crate::sync::SyncKind::Diverged);
        assert_eq!(
            result.sync.ancestor, None,
            "the session-scoped rung must find nothing"
        );
        let MergePrepOutcome::Ready { ancestor, .. } = result.outcome else {
            unreachable!("expected Ready");
        };
        assert_eq!(
            ancestor,
            Some((AncestorRung::Lineage, b"baseline content".to_vec()))
        );
    }

    /// Plan WP3 "Done when" (a): a diverged fixture's `MergePrep` reports
    /// `Diverged` and hands back both sides' actual bytes, not just hashes.
    #[test]
    fn merge_prep_on_a_diverged_document_returns_both_sides_bytes() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"theirs content");

        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = DocId(conn.last_insert_rowid());

        {
            let tx = conn.transaction().expect("tx");
            crate::journal::append_edit(
                &tx,
                session_id,
                SystemTime::now(),
                doc_id,
                &[rune_core::buffer::AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: "ours content".to_string(),
                }],
                &[],
                &[],
            )
            .expect("append_edit");
            tx.commit().expect("commit");
        }

        let result =
            merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");
        assert_eq!(result.sync.kind, crate::sync::SyncKind::Diverged);
        let MergePrepOutcome::Ready { ancestor, theirs } = result.outcome else {
            unreachable!("expected Ready");
        };
        let (theirs_obs, theirs_bytes) = theirs.expect("theirs must be present");
        assert_eq!(theirs_bytes, b"theirs content".to_vec());
        let _ = theirs_obs;
        assert_eq!(ancestor, None, "no prior ancestor-eligible sighting");
    }

    /// Plan WP3 "Done when" (b): a `DiskAhead` document (clean buffer, disk
    /// moved) returns the disk bytes as `theirs` with no ancestor divergence
    /// story needed for the fast path.
    #[test]
    fn merge_prep_on_a_disk_ahead_document_returns_theirs_bytes() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"");

        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = DocId(conn.last_insert_rowid());

        // A 'load' observation at seq 0 (ancestor-eligible), matching the
        // empty journal reconstruction — the buffer never changed, so any
        // later disk-only change is `DiskAhead`.
        {
            let tx = conn.transaction().expect("tx");
            let empty_hash = crate::blob::put_blob(&tx, b"").expect("seed empty blob");
            crate::observation::record_observation(
                &tx,
                doc_id,
                session_id,
                crate::observation::ObservationMeta {
                    blob_hash: &empty_hash,
                    seq: Some(0),
                    origin: ObsOrigin::Load,
                    confirmed: Confirmation::Unclassified,
                },
                &crate::observation::StatFacts {
                    mtime: Some("t".to_string()),
                    ..Default::default()
                },
                "t",
            )
            .expect("record load observation");
            tx.commit().expect("commit");
        }

        vfs.save_atomic(path, b"disk moved on").expect("overwrite");

        let result =
            merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");
        assert_eq!(result.sync.kind, crate::sync::SyncKind::DiskAhead);
        let MergePrepOutcome::Ready { theirs, .. } = result.outcome else {
            unreachable!("expected Ready");
        };
        let (_, theirs_bytes) = theirs.expect("theirs must be present");
        assert_eq!(theirs_bytes, b"disk moved on".to_vec());
    }

    /// Task WP-A(2ii): a persistently unconfirmed disk state (the file
    /// keeps changing across every re-probe attempt) must never be served as
    /// Theirs — `merge_prep` reports `MergePrepOutcome::Unstable`, never an
    /// empty/unstable Theirs. Driven through `Mem::mutate_after_next_stat`,
    /// re-armed after each of the bounded retry attempts so the bracket
    /// never settles.
    #[test]
    fn merge_prep_reports_unstable_when_disk_keeps_disagreeing_with_itself() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"theirs content");

        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = DocId(conn.last_insert_rowid());

        {
            let tx = conn.transaction().expect("tx");
            crate::journal::append_edit(
                &tx,
                session_id,
                SystemTime::now(),
                doc_id,
                &[rune_core::buffer::AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: "ours content".to_string(),
                }],
                &[],
                &[],
            )
            .expect("append_edit");
            tx.commit().expect("commit");
        }

        // Perpetual churn: the disk never stops moving, so no re-probe
        // attempt's own bracket can ever settle.
        vfs.set_churning(path, true);

        let result =
            merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");
        assert_eq!(
            result.outcome,
            MergePrepOutcome::Unstable,
            "a persistently unconfirmed disk must report unstable"
        );
    }

    /// Review fix F4: an untitled document (`path` is empty — `probe::probe`
    /// degrades to a pure `sync::sync` with nothing to read from disk at
    /// all) with no recorded observation has no `theirs` version — `Clean`
    /// via `classify_sync`'s `theirs: None` branch. `theirs` comes back
    /// `None` too, not an empty `Vec`/`0` sentinel standing in for "absent".
    #[test]
    fn merge_prep_on_an_untitled_document_returns_no_theirs() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");

        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed untitled doc");
        let doc_id = DocId(conn.last_insert_rowid());

        let result =
            merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");
        assert_eq!(result.sync.kind, crate::sync::SyncKind::Clean);
        let MergePrepOutcome::Ready { theirs, .. } = result.outcome else {
            unreachable!("expected Ready");
        };
        assert_eq!(theirs, None);
    }

    /// A legitimate external tool condensing a large file to a fraction of
    /// its size, in one atomic publish (never a still-mutating churn), must
    /// resolve within `merge_prep`'s own bounded re-probes rather than
    /// staying `unstable` forever: the first internal probe sights the
    /// shrink as an unconfirmed hypothesis, and the second — reading the
    /// now-quiescent disk again — sees byte-identical content and confirms
    /// it, so `merge_prep` serves the shrunk content as Theirs.
    #[test]
    fn merge_prep_serves_a_legitimate_shrink_confirmed_by_a_second_identical_sighting() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        let long_content = b"a very long paragraph of real disk content, unabridged";
        publish(&vfs, path, long_content);

        let loaded = crate::load::load(
            &mut conn,
            &vfs,
            session_id,
            &|_, _| false,
            path,
            SystemTime::now(),
        )
        .expect("load");
        let doc_id = loaded.doc_id;

        vfs.save_atomic(path, b"short").expect("publish shrink");

        let result =
            merge_prep(&mut conn, &vfs, session_id, doc_id, SystemTime::now()).expect("merge_prep");

        assert_ne!(
            result.outcome,
            MergePrepOutcome::Unstable,
            "a stable, legitimate shrink must resolve, not stay unstable forever"
        );
        assert_eq!(result.sync.kind, crate::sync::SyncKind::DiskAhead);
        let MergePrepOutcome::Ready { theirs, .. } = result.outcome else {
            unreachable!("expected Ready");
        };
        let (_, theirs_bytes) = theirs.expect("theirs must be present");
        assert_eq!(theirs_bytes, b"short".to_vec());
    }
}
