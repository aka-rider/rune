//! Identifier/observation-recording and column-versioning-lookup tests —
//! split out of `schema_tests.rs` to keep that file under the file-size
//! ceiling.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::SystemTime;

use super::*;
use crate::confirmation::Confirmation;
use crate::ids::{DocId, SessionId};
use crate::obs_origin::ObsOrigin;
use crate::observation::{self, ObservationMeta, StatFacts};

fn seed_minimal(conn: &Connection) -> (DocId, SessionId, String) {
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    let doc_id = DocId(conn.last_insert_rowid());
    let session_id =
        crate::session::establish_session(conn, SystemTime::now()).expect("seed session");
    let hash = crate::blob::put_blob(conn, b"content").expect("seed blob");
    (doc_id, session_id, hash)
}

#[test]
fn record_observation_without_confirmed_reads_back_none() {
    let mut conn = Connection::open_in_memory().expect("open");
    apply(&mut conn).expect("apply");
    let (doc_id, session_id, hash) = seed_minimal(&conn);

    let tx = conn.transaction().expect("tx");
    let obs_id = observation::record_observation(
        &tx,
        doc_id,
        session_id,
        ObservationMeta {
            blob_hash: &hash,
            seq: None,
            origin: ObsOrigin::Probe,
            confirmed: Confirmation::Unclassified,
        },
        &StatFacts {
            size: Some(1),
            mtime: Some("t".to_string()),
            ..Default::default()
        },
        "t",
    )
    .expect("record observation");
    let obs = observation::get_observation(&tx, obs_id).expect("read back");
    assert_eq!(obs.confirmed, Confirmation::Unclassified);
    tx.commit().expect("commit");
}

#[test]
fn record_observation_with_confirmed_round_trips_both_values() {
    let mut conn = Connection::open_in_memory().expect("open");
    apply(&mut conn).expect("apply");
    let (doc_id, session_id, hash) = seed_minimal(&conn);

    let tx = conn.transaction().expect("tx");
    let stat = StatFacts {
        size: Some(1),
        mtime: Some("t".to_string()),
        ..Default::default()
    };

    let true_id = observation::record_observation(
        &tx,
        doc_id,
        session_id,
        ObservationMeta {
            blob_hash: &hash,
            seq: None,
            origin: ObsOrigin::Probe,
            confirmed: Confirmation::Confirmed,
        },
        &stat,
        "t",
    )
    .expect("record confirmed=true");
    let false_id = observation::record_observation(
        &tx,
        doc_id,
        session_id,
        ObservationMeta {
            blob_hash: &hash,
            seq: None,
            origin: ObsOrigin::Probe,
            confirmed: Confirmation::Unconfirmed,
        },
        &stat,
        "t",
    )
    .expect("record confirmed=false");

    assert_eq!(
        observation::get_observation(&tx, true_id)
            .expect("read true")
            .confirmed,
        Confirmation::Confirmed
    );
    assert_eq!(
        observation::get_observation(&tx, false_id)
            .expect("read false")
            .confirmed,
        Confirmation::Unconfirmed
    );
    tx.commit().expect("commit");
}

#[test]
fn column_source_segment_finds_only_the_named_columns_own_definition() {
    let create_sql = "CREATE TABLE t (a INTEGER, b TEXT NOT NULL, c INTEGER CHECK(c > 0))";
    assert_eq!(
        column_source_segment(create_sql, "b").as_deref(),
        Some("b TEXT NOT NULL")
    );
    assert_eq!(
        column_source_segment(create_sql, "c").as_deref(),
        Some("c INTEGER CHECK(c > 0)")
    );
    assert_eq!(column_source_segment(create_sql, "missing"), None);
}

#[test]
fn a_check_constraint_on_the_column_is_flagged_as_unreproducible() {
    assert!(column_carries_an_unreproducible_constraint(
        "c INTEGER CHECK(c > 0)"
    ));
    assert!(!column_carries_an_unreproducible_constraint("c INTEGER"));
}

#[test]
fn column_source_segment_ignores_a_comma_sitting_inside_a_line_comment() {
    let create_sql =
        "CREATE TABLE t (a INTEGER, -- a comment, with a comma of its own\n b TEXT NOT NULL)";
    assert_eq!(
        column_source_segment(create_sql, "b").as_deref(),
        Some("b TEXT NOT NULL")
    );
}

#[test]
fn column_source_segment_locates_a_columns_own_definition_in_the_real_schema() {
    let canonical = Connection::open_in_memory().expect("open");
    canonical.execute_batch(SCHEMA).expect("apply real schema");
    let create_sql = table_create_sql(&canonical, "observations").expect("read real create sql");

    let segment = column_source_segment(&create_sql, "parent_a")
        .expect("a column's own definition must be found in the schema this crate ships");

    assert_eq!(segment, "parent_a INTEGER REFERENCES observations(id)");
}
