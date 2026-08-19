#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rusqlite::{Connection, Transaction};

use crate::ids::DocId;

pub(crate) fn open() -> Connection {
    crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(
        &crate::conn::memory_uri(),
    ))
    .expect("open")
}

pub(crate) fn insert_test_document(tx: &Transaction<'_>) -> DocId {
    tx.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
        [],
    )
    .expect("insert document");
    DocId(tx.last_insert_rowid())
}

pub(crate) fn always_dead(_pid: i64, _started_at: &str) -> bool {
    false
}
