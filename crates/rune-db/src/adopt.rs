//! The Adoption Contract — the only four verbs allowed to move
//! `session_documents.saved_obs` (see the `observation` module doc):
//! `materialize::commit_save` (inlined, since its move must commit in the
//! SAME tx as its re-Bind), [`adopt_equal`], [`resolve_adopt`], and
//! [`resolve_abandon`].

use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::Error;
use crate::obs_origin::ObsOrigin;
use crate::observation::{self, Observation, ObservationMeta, StatFacts};
use crate::retry;

#[cfg(test)]
use crate::confirmation::Confirmation;
use crate::ids::{BlobHash, DocId, ObsId, SessionId};

/// The shared one-tx BODY behind every path that moves `saved_obs` to a
/// NEWLY-inserted observation: a fresh row is inserted (tagged
/// `session_id`), `parent_a` is set to whatever `session_id`'s `saved_obs`
/// held immediately before (`None` if none), `parent_b` is the caller's own
/// second lineage edge (the disk-side observation a resolve/merge or a
/// racing save reconciled against — `None` for an adoption with nothing to
/// reconcile), and `session_documents.saved_obs` advances to the new row.
/// Runs entirely inside the CALLER's already-open tx — `materialize::
/// commit_save` calls this directly inside its own save tx (observation +
/// saved_obs + re-Bind must commit atomically together there);
/// [`record_adoption`] below wraps it in its own transaction for standalone
/// callers.
pub(crate) fn record_adoption_tx(
    tx: &Transaction<'_>,
    doc_id: DocId,
    session_id: SessionId,
    meta: ObservationMeta<'_>,
    stat: &StatFacts,
    at: &str,
    parent_b: Option<ObsId>,
) -> Result<Observation, Error> {
    let parent_a: Option<ObsId> = tx
        .query_row(
            "SELECT saved_obs FROM session_documents WHERE session_id=?1 AND doc_id=?2",
            params![session_id, doc_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();

    let new_id = observation::insert_observation_row(
        tx,
        doc_id,
        session_id,
        meta,
        stat,
        at,
        observation::ParentEdges {
            a: parent_a,
            b: parent_b,
        },
    )?;

    tx.execute(
        "INSERT INTO session_documents(session_id, doc_id, saved_obs) VALUES(?1,?2,?3) \
         ON CONFLICT(session_id, doc_id) DO UPDATE SET saved_obs=excluded.saved_obs",
        params![session_id, doc_id, new_id],
    )?;

    Ok(Observation {
        id: new_id,
        doc_id,
        session_id,
        blob_hash: BlobHash(meta.blob_hash.to_string()),
        seq: meta.seq,
        size: stat.size,
        mtime: stat.mtime.clone(),
        inode: stat.inode,
        device: stat.device,
        nlink: stat.nlink,
        origin: meta.origin,
        parent_a,
        parent_b,
        at: at.to_string(),
        confirmed: meta.confirmed,
    })
}

/// The shared one-tx primitive behind every STANDALONE path that moves
/// `saved_obs` to a newly-inserted observation — [`adopt_equal`],
/// [`resolve_adopt`], and `load::load`'s own first-sighting/heal-adopt
/// cases.
pub(crate) fn record_adoption(
    conn: &mut Connection,
    doc_id: DocId,
    session_id: SessionId,
    meta: ObservationMeta<'_>,
    stat: &StatFacts,
    now: SystemTime,
    parent_b: Option<ObsId>,
) -> Result<Observation, Error> {
    let at = crate::session::format_rfc3339_nanos(now);
    retry::with_retry(conn, |tx| {
        record_adoption_tx(tx, doc_id, session_id, meta, stat, &at, parent_b)
    })
}

/// Promotes a bare sighting (`obs`, already recorded — e.g. `probe::probe`'s
/// bare `'probe'` observation) to a genuine adoption when its content
/// hash-equals the journal-head reconstruction: a NEW `origin='resolve'`
/// observation is inserted, correlated to `head_seq` (making it
/// ancestor-eligible, unlike the bare sighting it promotes), and
/// `saved_obs` advances to it. The crash-between-swap-and-ack recovery
/// path — never used for an ordinary divergence.
pub(crate) fn adopt_equal(
    conn: &mut Connection,
    session_id: SessionId,
    doc_id: DocId,
    obs: ObsId,
    head_seq: i64,
    now: SystemTime,
) -> Result<Observation, Error> {
    let source = retry::with_retry(conn, |tx| observation::get_observation(tx, obs))?;
    let stat = source.stat();
    record_adoption(
        conn,
        doc_id,
        session_id,
        ObservationMeta {
            blob_hash: source.blob_hash.as_str(),
            seq: Some(head_seq),
            origin: ObsOrigin::Resolve,
            // Copy-forward of `source`'s own confirmed status — this
            // promotes a bare sighting to a genuine adoption, it does not
            // re-read disk, so it has no fresher fact to derive one from.
            confirmed: source.confirmed,
        },
        &stat,
        now,
        None,
    )
}

/// Commits a [D]iscard/[M]erge resolution (or an explicit hash-equality
/// adopt): re-tags `obs` as `origin='resolve'`, correlated to `edit_seq`
/// (the seq of the journaled replace-all/merge-entry edit that resolved
/// it), and advances `saved_obs` to it. Undo past `edit_seq` moves the
/// journal position below this resolve observation, so `ancestor_at`
/// automatically stops finding it and `sync` reports `Diverged` again — the
/// guard re-raises with no bespoke unwind logic.
///
/// `edit_seq: None` means the caller (the merge-entry TUI flow, plan WP3
/// Gotchas `[B3]`) could not learn the exact durable seq of its own install
/// edit synchronously — `Store::append_edit`'s ack is asynchronous, and
/// `update` may never block on it. The writer thread processes every op in
/// strict FIFO order, so by the time THIS op runs, that install edit has
/// already committed; resolving `journal::current_seq` fresh, inside this
/// same transaction, yields exactly that edit's seq (the journal head at
/// this instant) without the caller ever needing to wait for its ack. The
/// new resolve row's `parent_b` is set to `obs` itself — the disk-side
/// observation this resolution reconciled against — completing the
/// two-parent join alongside `parent_a`'s prior `saved_obs` baseline.
pub(crate) fn resolve_adopt(
    conn: &mut Connection,
    session_id: SessionId,
    doc_id: DocId,
    obs: ObsId,
    edit_seq: Option<i64>,
    now: SystemTime,
) -> Result<Observation, Error> {
    let source = retry::with_retry(conn, |tx| observation::get_observation(tx, obs))?;
    let stat = source.stat();
    let seq = match edit_seq {
        Some(seq) => seq,
        None => {
            retry::with_retry(conn, |tx| {
                crate::journal::current_seq(tx, session_id, doc_id)
            })?
            .0
        }
    };
    record_adoption(
        conn,
        doc_id,
        session_id,
        ObservationMeta {
            blob_hash: source.blob_hash.as_str(),
            seq: Some(seq),
            origin: ObsOrigin::Resolve,
            // Copy-forward of `source`'s own confirmed status — see
            // `adopt_equal`'s identical reasoning.
            confirmed: source.confirmed,
        },
        &stat,
        now,
        Some(obs),
    )
}

pub(crate) fn resolve_abandon(
    conn: &mut Connection,
    session_id: SessionId,
    doc_id: DocId,
) -> Result<(), Error> {
    retry::with_retry(conn, |tx| {
        let current: Option<ObsId> = tx
            .query_row(
                "SELECT saved_obs FROM session_documents WHERE session_id=?1 AND doc_id=?2",
                params![session_id, doc_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let Some(current) = current else {
            return Ok(()); // no session_documents row, or saved_obs NULL — nothing adopted yet
        };

        let (parent_a, origin): (Option<ObsId>, ObsOrigin) = tx.query_row(
            "SELECT parent_a, origin FROM observations WHERE id=?1",
            params![current],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        if !matches!(origin, ObsOrigin::Resolve) {
            return Err(Error::Invalid(format!(
                "resolve abandon doc {doc_id}: baseline observation {current} has origin {origin:?}, not a resolve adoption — refusing to delete it"
            )));
        }

        tx.execute(
            "UPDATE session_documents SET saved_obs=?1 WHERE session_id=?2 AND doc_id=?3",
            params![parent_a, session_id, doc_id],
        )?;

        let still_referenced: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM session_documents WHERE saved_obs=?1
                 UNION ALL
                 SELECT 1 FROM observations WHERE parent_a=?1 OR parent_b=?1
                 UNION ALL
                 SELECT 1 FROM merges WHERE base_obs=?1 OR theirs_obs=?1
             )",
            params![current],
            |r| r.get(0),
        )?;
        if !still_referenced {
            tx.execute("DELETE FROM observations WHERE id=?1", params![current])?;
        }
        Ok(())
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_support::open;

    fn seed_doc(conn: &Connection) -> DocId {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        DocId(conn.last_insert_rowid())
    }

    /// `observations.blob_hash` is FK-constrained to `blobs.hash` — every
    /// hand-seeded observation in these tests needs a real blob row first.
    fn seed_blob(conn: &Connection, content: &str) -> String {
        crate::blob::put_blob(conn, content.as_bytes()).expect("seed blob")
    }

    fn test_stat() -> StatFacts {
        StatFacts {
            size: Some(1),
            mtime: Some("t".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn resolve_abandon_restores_exact_prior_baseline() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc(&conn);
        let hash_1 = seed_blob(&conn, "content 1");

        // First: an ordinary save baseline.
        let first = record_adoption(
            &mut conn,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash_1,
                seq: Some(1),
                origin: ObsOrigin::Save,
                confirmed: Confirmation::Unclassified,
            },
            &test_stat(),
            SystemTime::now(),
            None,
        )
        .expect("first adoption");

        // Then: a resolve adoption on top of it.
        let resolved = adopt_equal(
            &mut conn,
            session_id,
            doc_id,
            first.id,
            2,
            SystemTime::now(),
        )
        .expect("adopt_equal");
        assert_eq!(resolved.parent_a, Some(first.id));

        resolve_abandon(&mut conn, session_id, doc_id).expect("resolve_abandon");

        let current: Option<ObsId> = conn
            .query_row(
                "SELECT saved_obs FROM session_documents WHERE session_id=?1 AND doc_id=?2",
                params![session_id, doc_id],
                |r| r.get(0),
            )
            .expect("read saved_obs");
        assert_eq!(
            current,
            Some(first.id),
            "must restore the exact prior baseline"
        );

        let deleted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE id=?1",
                params![resolved.id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(deleted, 0, "the resolve observation row must be gone");
    }

    #[test]
    fn resolve_abandon_refuses_a_non_resolve_baseline() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc(&conn);
        let hash_1 = seed_blob(&conn, "content 1");

        record_adoption(
            &mut conn,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash_1,
                seq: Some(1),
                origin: ObsOrigin::Save,
                confirmed: Confirmation::Unclassified,
            },
            &test_stat(),
            SystemTime::now(),
            None,
        )
        .expect("save adoption");

        let err = resolve_abandon(&mut conn, session_id, doc_id).expect_err("must refuse");
        assert!(matches!(err, Error::Invalid(_)));
    }

    #[test]
    fn resolve_abandon_survives_a_later_observation_chained_to_it() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc(&conn);
        let hash_1 = seed_blob(&conn, "content 1");

        let first = record_adoption(
            &mut conn,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash_1,
                seq: Some(1),
                origin: ObsOrigin::Save,
                confirmed: Confirmation::Unclassified,
            },
            &test_stat(),
            SystemTime::now(),
            None,
        )
        .expect("first adoption");

        let resolved = adopt_equal(
            &mut conn,
            session_id,
            doc_id,
            first.id,
            2,
            SystemTime::now(),
        )
        .expect("adopt_equal");

        conn.execute(
            "INSERT INTO observations(doc_id, session_id, blob_hash, seq, origin, parent_a, at) \
             VALUES(?1,?2,?3,?4,'probe',?5,'x')",
            params![doc_id, session_id, hash_1, 3, resolved.id],
        )
        .expect("seed chained observation");

        resolve_abandon(&mut conn, session_id, doc_id).expect("resolve_abandon");

        let current: Option<ObsId> = conn
            .query_row(
                "SELECT saved_obs FROM session_documents WHERE session_id=?1 AND doc_id=?2",
                params![session_id, doc_id],
                |r| r.get(0),
            )
            .expect("read saved_obs");
        assert_eq!(current, Some(first.id));

        let survives: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE id=?1",
                params![resolved.id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            survives, 1,
            "a still-referenced resolve row must survive as a lineage ancestor"
        );
    }

    #[test]
    fn resolve_abandon_survives_a_merges_row_referencing_it() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc(&conn);
        let hash_1 = seed_blob(&conn, "content 1");

        let first = record_adoption(
            &mut conn,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash_1,
                seq: Some(1),
                origin: ObsOrigin::Save,
                confirmed: Confirmation::Unclassified,
            },
            &test_stat(),
            SystemTime::now(),
            None,
        )
        .expect("first adoption");

        let resolved = adopt_equal(
            &mut conn,
            session_id,
            doc_id,
            first.id,
            2,
            SystemTime::now(),
        )
        .expect("adopt_equal");

        conn.execute(
            "INSERT INTO merges(doc_id, session_id, base_obs, theirs_obs, marker_hash, blocks, state, created_at) \
             VALUES(?1,?2,?3,?3,?4,'[]','active','x')",
            params![doc_id, session_id, resolved.id, hash_1],
        )
        .expect("seed merges row");

        resolve_abandon(&mut conn, session_id, doc_id).expect("resolve_abandon");

        let current: Option<ObsId> = conn
            .query_row(
                "SELECT saved_obs FROM session_documents WHERE session_id=?1 AND doc_id=?2",
                params![session_id, doc_id],
                |r| r.get(0),
            )
            .expect("read saved_obs");
        assert_eq!(current, Some(first.id));

        let survives: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE id=?1",
                params![resolved.id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            survives, 1,
            "a resolve row referenced by a merges row must survive"
        );
    }

    #[test]
    fn resolve_abandon_on_a_document_with_no_baseline_is_a_safe_no_op() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc(&conn);

        resolve_abandon(&mut conn, session_id, doc_id).expect("must be a no-op, not an error");
    }
}
