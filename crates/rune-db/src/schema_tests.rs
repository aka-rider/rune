#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

const OBSERVATIONS_BEFORE_CONFIRMED_COLUMN: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
    id           INTEGER PRIMARY KEY,
    path         TEXT    NOT NULL DEFAULT '',
    inode        INTEGER,
    device       INTEGER,
    kind         TEXT    NOT NULL DEFAULT 'file' CHECK(kind IN ('file','scratch','chat')),
    created_at   TEXT    NOT NULL,
    last_seen_at TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS blobs (
    hash    TEXT PRIMARY KEY,
    content BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    id              INTEGER PRIMARY KEY,
    pid             INTEGER NOT NULL,
    proc_started_at TEXT    NOT NULL,
    opened_at       TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS observations (
    id         INTEGER PRIMARY KEY,
    doc_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    session_id INTEGER NOT NULL REFERENCES sessions(id),
    blob_hash  TEXT    NOT NULL REFERENCES blobs(hash),
    seq        INTEGER,
    size       INTEGER,
    mtime      TEXT,
    inode      INTEGER,
    device     INTEGER,
    nlink      INTEGER,
    origin     TEXT    NOT NULL CHECK(origin IN ('load','save','watch','probe','resolve','swap')),
    parent_a   INTEGER REFERENCES observations(id),
    parent_b   INTEGER REFERENCES observations(id),
    at         TEXT    NOT NULL
);
"#;

#[test]
fn additive_column_lands_in_place_old_rows_read_null_and_a_second_apply_is_a_noop() {
    let mut conn = Connection::open_in_memory().expect("open");
    conn.execute_batch(OBSERVATIONS_BEFORE_CONFIRMED_COLUMN)
        .expect("apply the pre-existing shape");
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    let doc_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES (1, 'x', 'x')",
        [],
    )
    .expect("seed session");
    let session_id = conn.last_insert_rowid();
    conn.execute("INSERT INTO blobs(hash, content) VALUES ('h', x'00')", [])
        .expect("seed blob");
    conn.execute(
        "INSERT INTO observations(doc_id, session_id, blob_hash, origin, at) VALUES (?1, ?2, 'h', 'probe', 'x')",
        rusqlite::params![doc_id, session_id],
    )
    .expect("seed a row written before the confirmed column existed");

    apply(&mut conn).expect("apply reconciles the missing column");

    let read_confirmed = |conn: &Connection| -> Option<i64> {
        conn.query_row(
            "SELECT confirmed FROM observations WHERE doc_id = ?1",
            [doc_id],
            |row| row.get(0),
        )
        .expect("read back confirmed")
    };
    assert_eq!(read_confirmed(&conn), None);

    apply(&mut conn).expect("a second apply against an already-reconciled file is a no-op");
    assert_eq!(read_confirmed(&conn), None);
}

const OBSERVATIONS_MISSING_REQUIRED_ORIGIN_COLUMN: &str = r#"
CREATE TABLE IF NOT EXISTS observations (
    id         INTEGER PRIMARY KEY,
    doc_id     INTEGER NOT NULL,
    session_id INTEGER NOT NULL,
    blob_hash  TEXT    NOT NULL,
    seq        INTEGER,
    size       INTEGER,
    mtime      TEXT,
    inode      INTEGER,
    device     INTEGER,
    nlink      INTEGER,
    parent_a   INTEGER,
    parent_b   INTEGER,
    at         TEXT    NOT NULL
);
"#;

#[test]
fn missing_not_null_column_without_a_default_is_refused_not_silently_added() {
    let mut conn = Connection::open_in_memory().expect("open");
    conn.execute_batch(OBSERVATIONS_MISSING_REQUIRED_ORIGIN_COLUMN)
        .expect("apply the shape missing a required column");

    let err =
        apply(&mut conn).expect_err("a NOT NULL column with no default is not an additive change");
    let message = err.to_string();
    assert!(message.contains("observations"));
    assert!(message.contains("origin"));
}

const OBSERVATIONS_BEFORE_PARENT_A_COLUMN: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
    id           INTEGER PRIMARY KEY,
    path         TEXT    NOT NULL DEFAULT '',
    inode        INTEGER,
    device       INTEGER,
    kind         TEXT    NOT NULL DEFAULT 'file' CHECK(kind IN ('file','scratch','chat')),
    created_at   TEXT    NOT NULL,
    last_seen_at TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS blobs (
    hash    TEXT PRIMARY KEY,
    content BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    id              INTEGER PRIMARY KEY,
    pid             INTEGER NOT NULL,
    proc_started_at TEXT    NOT NULL,
    opened_at       TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS observations (
    id         INTEGER PRIMARY KEY,
    doc_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    session_id INTEGER NOT NULL REFERENCES sessions(id),
    blob_hash  TEXT    NOT NULL REFERENCES blobs(hash),
    seq        INTEGER,
    size       INTEGER,
    mtime      TEXT,
    inode      INTEGER,
    device     INTEGER,
    nlink      INTEGER,
    origin     TEXT    NOT NULL CHECK(origin IN ('load','save','watch','probe','resolve','swap')),
    parent_b   INTEGER REFERENCES observations(id),
    at         TEXT    NOT NULL,
    confirmed  INTEGER
);
"#;

#[test]
fn additive_column_carrying_a_foreign_key_enforces_it_after_reconciliation() {
    let mut conn = Connection::open_in_memory().expect("open");
    conn.execute_batch(OBSERVATIONS_BEFORE_PARENT_A_COLUMN)
        .expect("apply the shape missing parent_a");
    conn.pragma_update(None, "foreign_keys", true)
        .expect("enable foreign key enforcement");
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    let doc_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES (1, 'x', 'x')",
        [],
    )
    .expect("seed session");
    let session_id = conn.last_insert_rowid();
    conn.execute("INSERT INTO blobs(hash, content) VALUES ('h', x'00')", [])
        .expect("seed blob");

    apply(&mut conn).expect("apply adds parent_a with its foreign key intact");

    let result = conn.execute(
        "INSERT INTO observations(doc_id, session_id, blob_hash, origin, at, parent_a) VALUES (?1, ?2, 'h', 'probe', 'x', 999999)",
        rusqlite::params![doc_id, session_id],
    );
    assert!(
        result.is_err(),
        "a parent_a value with no matching observations row must be rejected by the foreign key the reconciled column carries, not silently accepted"
    );
}

const SCHEMA_BEFORE_COMMAND_HISTORY_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
    id           INTEGER PRIMARY KEY,
    path         TEXT    NOT NULL DEFAULT '',
    inode        INTEGER,
    device       INTEGER,
    kind         TEXT    NOT NULL DEFAULT 'file' CHECK(kind IN ('file','scratch','chat')),
    created_at   TEXT    NOT NULL,
    last_seen_at TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS search_history (
    query        TEXT PRIMARY KEY,
    last_used_at TEXT NOT NULL
);
"#;

#[test]
fn apply_on_a_file_missing_command_history_creates_it_without_disturbing_other_rows() {
    let mut conn = Connection::open_in_memory().expect("open");
    conn.execute_batch(SCHEMA_BEFORE_COMMAND_HISTORY_TABLE)
        .expect("apply the shape missing command_history");
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('x', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    conn.execute(
        "INSERT INTO search_history(query, last_used_at) VALUES ('hello', 't')",
        [],
    )
    .expect("seed search_history");

    apply(&mut conn).expect("apply creates the missing table");

    let doc_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .expect("count documents");
    assert_eq!(doc_count, 1);
    let search_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM search_history", [], |r| r.get(0))
        .expect("count search_history");
    assert_eq!(search_count, 1);
    let command_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM command_history", [], |r| r.get(0))
        .expect("count command_history");
    assert_eq!(command_count, 0);

    apply(&mut conn).expect("a second apply against an already-reconciled file is a no-op");
    let doc_count_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .expect("count documents again");
    assert_eq!(doc_count_after, 1);
}

const SCHEMA_BEFORE_EVENTS_KIND_COLUMN: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
    id           INTEGER PRIMARY KEY,
    path         TEXT    NOT NULL DEFAULT '',
    inode        INTEGER,
    device       INTEGER,
    kind         TEXT    NOT NULL DEFAULT 'file' CHECK(kind IN ('file','scratch','chat')),
    created_at   TEXT    NOT NULL,
    last_seen_at TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    id              INTEGER PRIMARY KEY,
    pid             INTEGER NOT NULL,
    proc_started_at TEXT    NOT NULL,
    opened_at       TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
    seq            INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id         INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    session_id     INTEGER NOT NULL REFERENCES sessions(id)  ON DELETE CASCADE,
    edits          BLOB NOT NULL,
    cursors_before BLOB,
    cursors_after  BLOB,
    at             TEXT NOT NULL
);
"#;

#[test]
fn apply_on_a_file_missing_the_events_kind_column_adds_it_without_disturbing_other_rows() {
    let mut conn = Connection::open_in_memory().expect("open");
    conn.execute_batch(SCHEMA_BEFORE_EVENTS_KIND_COLUMN)
        .expect("apply the shape missing events.kind");
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('x', 'x', 'x')",
        [],
    )
    .expect("seed doc");
    let doc_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES (1, 'x', 'x')",
        [],
    )
    .expect("seed session");
    let session_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO events(doc_id, session_id, edits, cursors_before, cursors_after, at) \
         VALUES (?1, ?2, '[]', '[]', '[]', 'x')",
        rusqlite::params![doc_id, session_id],
    )
    .expect("seed a row written before the kind column existed");

    apply(&mut conn).expect("apply reconciles the missing column");

    let read_kind = |conn: &Connection| -> Option<String> {
        conn.query_row(
            "SELECT kind FROM events WHERE doc_id = ?1",
            [doc_id],
            |row| row.get(0),
        )
        .expect("read back kind")
    };
    assert_eq!(read_kind(&conn), None);

    let doc_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .expect("count documents");
    assert_eq!(doc_count, 1);
    let event_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .expect("count events");
    assert_eq!(event_count, 1);

    apply(&mut conn).expect("a second apply against an already-reconciled file is a no-op");
    assert_eq!(read_kind(&conn), None);
    let doc_count_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
        .expect("count documents again");
    assert_eq!(doc_count_after, 1);
}

#[test]
fn the_shipped_schema_ddl_carries_no_line_comments() {
    assert!(
        !SCHEMA.contains("--"),
        "SCHEMA must stay comment-free: sqlite_master preserves DDL comments \
         verbatim, and the column parser's real-schema test only smoke-checks \
         a comment-free shape — knowledge belongs in module docs and the \
         constitution, not the DDL"
    );
}

#[test]
fn add_column_treats_a_concurrently_added_duplicate_column_as_a_successful_no_op() {
    let dir = crate::conn::test_temp_dir("schema-race");
    let path = dir.join("race.db");

    {
        let setup = Connection::open(&path).expect("open setup connection");
        setup
            .execute_batch(OBSERVATIONS_BEFORE_CONFIRMED_COLUMN)
            .expect("apply the pre-existing shape");
    }

    let winner = Connection::open(&path).expect("open winner connection");
    winner
        .execute_batch("ALTER TABLE observations ADD COLUMN confirmed INTEGER")
        .expect("winner adds the column first, simulating the process that won the race");

    let canonical = Connection::open_in_memory().expect("open canonical");
    canonical.execute_batch(SCHEMA).expect("apply real schema");
    let column = table_columns(&canonical, "observations")
        .expect("read canonical columns")
        .into_iter()
        .find(|c| c.name == "confirmed")
        .expect("confirmed column exists in the canonical schema");

    let loser = Connection::open(&path).expect("open loser connection");
    let result = add_column(&loser, "observations", &column, None);

    assert!(
        result.is_ok(),
        "a concurrent winner adding the same column first must not degrade this \
         session to the in-memory recovery rung: {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
