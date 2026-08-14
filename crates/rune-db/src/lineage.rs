//! The shared lineage primitive over `observations.parent_a`/`parent_b`:
//! every adoption records the CAS baseline it replaced, a resolve/merge
//! records the disk-side observation it reconciled against, and a CONFIRMED
//! fresh sighting that differs in hash from what was previously newest
//! records that prior newest (`Observation::parent_a`/`parent_b` carry the
//! per-edge meanings) — two edges per row at most, forming a DAG rather
//! than v1's single-parent forest. [`common_ancestor`] is how that DAG is
//! queried: the merge-prep ancestor ladder's own three-way base.

use rusqlite::{OptionalExtension, Transaction, params};

use crate::Error;
use crate::ids::ObsId;
#[cfg(test)]
use crate::ids::{DocId, SessionId};
use crate::observation::{self, Observation};

/// The recursive-CTE body [`common_ancestor`] runs once per side:
/// starting from `param` (a bound-parameter placeholder), walks every
/// reachable `parent_a`/`parent_b` edge upward, self-joining against
/// `cte_name` (the enclosing `WITH RECURSIVE` clause's own name — each
/// caller names its own, so two independent walks in one query never
/// collide). Every parent id a row can carry always refers to an
/// EARLIER-inserted row (the FK target already existed when the edge was
/// recorded), so the walk is acyclic by construction — no hop bound is
/// needed the way v1's single-parent walk carried one.
fn ancestors_of_clause(cte_name: &str, param: &str) -> String {
    format!(
        "SELECT {param}
         UNION
         SELECT o.parent_a FROM observations o JOIN {cte_name} ON o.id = {cte_name}.id WHERE o.parent_a IS NOT NULL
         UNION
         SELECT o.parent_b FROM observations o JOIN {cte_name} ON o.id = {cte_name}.id WHERE o.parent_b IS NOT NULL"
    )
}

/// The closest common ancestor of `a` and `b`: the full ancestor sets of
/// each (via [`ancestors_of_clause`]) intersected, then the intersection's
/// member with the LARGEST id — ids are assigned in insertion order, and a
/// parent edge always points at an earlier id, so within either ancestor
/// set a larger id is always strictly closer to that set's own starting
/// point; the largest id common to both sets is therefore the nearest node
/// ancestral to both. `None` when the two ancestor sets never intersect at
/// all.
pub fn common_ancestor(
    tx: &Transaction<'_>,
    a: ObsId,
    b: ObsId,
) -> Result<Option<Observation>, Error> {
    let clause_a = ancestors_of_clause("anc_a", "?1");
    let clause_b = ancestors_of_clause("anc_b", "?2");
    let sql = format!(
        "WITH RECURSIVE anc_a(id) AS ({clause_a}), anc_b(id) AS ({clause_b}) \
         SELECT MAX(id) FROM anc_a WHERE id IN (SELECT id FROM anc_b)"
    );
    let found: Option<ObsId> = tx
        .query_row(&sql, params![a, b], |r| r.get(0))
        .optional()?
        .flatten();
    found.map_or(Ok(None), |id| {
        observation::get_observation(tx, id).map(Some)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::confirmation::Confirmation;
    use crate::obs_origin::ObsOrigin;
    use crate::observation::{ObservationMeta, ParentEdges, StatFacts};
    use rusqlite::Connection;
    use std::time::SystemTime;

    fn open() -> Connection {
        crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(
            &crate::conn::memory_uri(),
        ))
        .expect("open")
    }

    fn seed_doc(tx: &Transaction<'_>) -> DocId {
        tx.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        DocId(tx.last_insert_rowid())
    }

    fn seed_obs(
        tx: &Transaction<'_>,
        doc_id: DocId,
        session_id: SessionId,
        content: &str,
        parent_a: Option<ObsId>,
    ) -> ObsId {
        let hash = crate::blob::put_blob(tx, content.as_bytes()).expect("seed blob");
        observation::insert_observation_row(
            tx,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: None,
                origin: ObsOrigin::Probe,
                confirmed: Confirmation::Confirmed,
            },
            &StatFacts {
                size: Some(content.len() as i64),
                mtime: Some("t".to_string()),
                ..Default::default()
            },
            "t",
            ParentEdges {
                a: parent_a,
                b: None,
            },
        )
        .expect("seed observation")
    }

    #[test]
    fn common_ancestor_of_a_node_and_itself_is_itself() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let a = seed_obs(&tx, doc_id, session_id, "one", None);

        let found = common_ancestor(&tx, a, a)
            .expect("common_ancestor")
            .expect("some");
        assert_eq!(found.id, a);
        tx.commit().expect("commit");
    }

    #[test]
    fn common_ancestor_finds_a_direct_parent() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let root = seed_obs(&tx, doc_id, session_id, "one", None);
        let child = seed_obs(&tx, doc_id, session_id, "two", Some(root));

        let found = common_ancestor(&tx, root, child)
            .expect("common_ancestor")
            .expect("some");
        assert_eq!(
            found.id, root,
            "the root is its own child's closest ancestor"
        );
        tx.commit().expect("commit");
    }

    #[test]
    fn common_ancestor_finds_a_multi_hop_ancestor() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let root = seed_obs(&tx, doc_id, session_id, "one", None);
        let mid = seed_obs(&tx, doc_id, session_id, "two", Some(root));
        let leaf = seed_obs(&tx, doc_id, session_id, "three", Some(mid));

        let found = common_ancestor(&tx, root, leaf)
            .expect("common_ancestor")
            .expect("some");
        assert_eq!(found.id, root, "must walk past the intermediate hop");
        tx.commit().expect("commit");
    }

    #[test]
    fn common_ancestor_of_two_disconnected_roots_is_none() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let a = seed_obs(&tx, doc_id, session_id, "one", None);
        let b = seed_obs(&tx, doc_id, session_id, "two", None);

        let found = common_ancestor(&tx, a, b).expect("common_ancestor");
        assert_eq!(
            found, None,
            "two roots with no shared parent edge must not falsely intersect"
        );
        tx.commit().expect("commit");
    }

    #[test]
    fn common_ancestor_finds_a_shared_ancestor_of_two_diverged_branches() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let root = seed_obs(&tx, doc_id, session_id, "one", None);
        let branch_a = seed_obs(&tx, doc_id, session_id, "two-a", Some(root));
        let branch_b = seed_obs(&tx, doc_id, session_id, "two-b", Some(root));

        let found = common_ancestor(&tx, branch_a, branch_b)
            .expect("common_ancestor")
            .expect("some");
        assert_eq!(found.id, root);
        tx.commit().expect("commit");
    }

    #[test]
    fn common_ancestor_walks_the_second_parent_edge() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let a = seed_obs(&tx, doc_id, session_id, "one", None);
        let b = seed_obs(&tx, doc_id, session_id, "two", None);
        let hash = crate::blob::put_blob(&tx, b"joined").expect("seed blob");
        let joined = observation::insert_observation_row(
            &tx,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: None,
                origin: ObsOrigin::Resolve,
                confirmed: Confirmation::Confirmed,
            },
            &StatFacts {
                size: Some(6),
                mtime: Some("t".to_string()),
                ..Default::default()
            },
            "t",
            ParentEdges {
                a: Some(a),
                b: Some(b),
            },
        )
        .expect("seed two-parent join");

        for (edge, parent) in [("parent_a", a), ("parent_b", b)] {
            let found = common_ancestor(&tx, parent, joined)
                .expect("common_ancestor")
                .expect("some");
            assert_eq!(found.id, parent, "the walk must follow {edge}");
        }
        tx.commit().expect("commit");
    }
}
