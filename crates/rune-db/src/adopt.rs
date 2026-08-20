use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::Error;
use crate::obs_origin::ObsOrigin;
use crate::observation::{self, Observation, ObservationMeta, StatFacts};
use crate::retry;

#[cfg(test)]
use crate::confirmation::Confirmation;
use crate::ids::{BlobHash, DocId, ObsId, SessionId};

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
            confirmed: source.confirmed,
        },
        &stat,
        now,
        None,
    )
}

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
            return Ok(());
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
