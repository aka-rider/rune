//! The shared lineage-walk primitive over `observations.supersedes`: every
//! adoption records the CAS baseline it replaced, and every confirmed fresh
//! sighting that differs in hash from what was previously newest records
//! that prior newest (`observation.rs`'s own doc on `Observation::
//! supersedes`) — one edge per row, so the whole table forms a forest of
//! single-parent chains. [`common_ancestor`] is the one place that forest is
//! walked to answer "do these two observations share a recorded history",
//! consumed by `sync.rs`'s own-history echo check and `merge_prep.rs`'s
//! ancestor ladder alike, so the walk-and-bound behavior can never drift
//! between the two.

use rusqlite::{OptionalExtension, Transaction, params};

use crate::Error;
use crate::observation::{self, ObsId, Observation};

/// Bounds how many `supersedes` hops a lineage walk follows before giving up
/// — generous enough for any realistic session history, but bounded so a
/// pathological chain (or, in principle, a cycle a bug somewhere else
/// introduced) can never spin unboundedly. Mirrors `bracket.rs`'s own
/// bounded-retry convention.
const LINEAGE_WALK_MAX_HOPS: u32 = 256;

/// `id`'s own `supersedes` chain, closest first: `id` itself, then its
/// `supersedes`, then that row's `supersedes`, and so on, bounded by
/// [`LINEAGE_WALK_MAX_HOPS`]. A chain longer than the bound is truncated —
/// the caller reads a truncated walk finding no match as "no edge found
/// within the bound", never as proof no such edge exists at all.
fn lineage_chain(tx: &Transaction<'_>, id: ObsId) -> Result<Vec<ObsId>, Error> {
    let mut chain = Vec::new();
    let mut cur = Some(id);
    let mut hops = 0u32;
    while let Some(cur_id) = cur {
        if hops >= LINEAGE_WALK_MAX_HOPS {
            break;
        }
        chain.push(cur_id);
        cur = tx
            .query_row(
                "SELECT supersedes FROM observations WHERE id=?1",
                params![cur_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        hops += 1;
    }
    Ok(chain)
}

/// The closest common ancestor of `a` and `b` along their own `supersedes`
/// chains (each bounded by [`LINEAGE_WALK_MAX_HOPS`]) — `Some(a)` when `a`
/// is on `b`'s own chain (or the reverse), `None` when the two bounded walks
/// never intersect at all. Since every row has at most one `supersedes`
/// parent, the two chains are each a straight root-ward path, so the first
/// node encountered while walking `b` that also appears in `a`'s own chain
/// is exactly the closest node ancestral to both — the answer is the same
/// regardless of which side is walked first.
pub fn common_ancestor(
    tx: &Transaction<'_>,
    a: ObsId,
    b: ObsId,
) -> Result<Option<Observation>, Error> {
    let chain_a = lineage_chain(tx, a)?;
    let chain_b = lineage_chain(tx, b)?;
    let seen: std::collections::HashSet<ObsId> = chain_a.into_iter().collect();
    for id in chain_b {
        if seen.contains(&id) {
            return observation::get_observation(tx, id).map(Some);
        }
    }
    Ok(None)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::observation::{ObservationMeta, StatFacts};
    use rusqlite::Connection;
    use std::time::SystemTime;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn seed_doc(tx: &Transaction<'_>) -> i64 {
        tx.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        tx.last_insert_rowid()
    }

    fn seed_obs(
        tx: &Transaction<'_>,
        doc_id: i64,
        session_id: i64,
        content: &str,
        supersedes: Option<ObsId>,
    ) -> ObsId {
        let hash = crate::blob::put_blob(tx, content.as_bytes()).expect("seed blob");
        observation::insert_observation_row(
            tx,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: None,
                origin: "probe",
                confirmed: Some(true),
            },
            &StatFacts {
                size: content.len() as i64,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
            supersedes,
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
            "two roots with no shared supersedes edge must not falsely intersect"
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
}
