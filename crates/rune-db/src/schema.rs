use std::collections::HashSet;

use rusqlite::Connection;

use crate::Error;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
	id           INTEGER PRIMARY KEY,
	path         TEXT    NOT NULL DEFAULT '',
	inode        INTEGER,
	device       INTEGER,
	kind         TEXT    NOT NULL DEFAULT 'file' CHECK(kind IN ('file','scratch','chat')),
	created_at   TEXT    NOT NULL,
	last_seen_at TEXT    NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_inode ON documents(inode, device) WHERE inode IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_path  ON documents(path)           WHERE path != '';

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

CREATE TABLE IF NOT EXISTS session_documents (
	session_id  INTEGER NOT NULL REFERENCES sessions(id)  ON DELETE CASCADE,
	doc_id      INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
	current_seq INTEGER CHECK(current_seq IS NULL OR current_seq >= 0),
	saved_obs   INTEGER REFERENCES observations(id),
	PRIMARY KEY(session_id, doc_id)
);

CREATE TABLE IF NOT EXISTS snapshots (
	id         INTEGER PRIMARY KEY,
	doc_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
	session_id INTEGER NOT NULL REFERENCES sessions(id)  ON DELETE CASCADE,
	blob_hash  TEXT    NOT NULL REFERENCES blobs(hash),
	seq        INTEGER NOT NULL DEFAULT 0 CHECK(seq >= 0),
	created_at TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snapshots_doc     ON snapshots(doc_id, id);
CREATE INDEX IF NOT EXISTS idx_snapshots_session ON snapshots(session_id);

CREATE TABLE IF NOT EXISTS events (
	seq            INTEGER PRIMARY KEY AUTOINCREMENT,
	doc_id         INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
	session_id     INTEGER NOT NULL REFERENCES sessions(id)  ON DELETE CASCADE,
	edits          BLOB NOT NULL,
	cursors_before BLOB,
	cursors_after  BLOB,
	at             TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_doc     ON events(doc_id, seq);
CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id);

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
	parent_a INTEGER REFERENCES observations(id),
	parent_b INTEGER REFERENCES observations(id),
	at        TEXT    NOT NULL,
	confirmed INTEGER
);
CREATE INDEX IF NOT EXISTS idx_observations_doc ON observations(doc_id, id);

CREATE TABLE IF NOT EXISTS merges (
	id          INTEGER PRIMARY KEY,
	doc_id      INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
	session_id  INTEGER NOT NULL REFERENCES sessions(id),
	base_obs    INTEGER REFERENCES observations(id),
	theirs_obs  INTEGER NOT NULL REFERENCES observations(id),
	marker_hash TEXT    NOT NULL REFERENCES blobs(hash),
	blocks      TEXT    NOT NULL,
	state       TEXT    NOT NULL CHECK(state IN ('active','completed','abandoned')),
	created_at  TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_merges_doc ON merges(doc_id, id);

CREATE TABLE IF NOT EXISTS search_history (
	query        TEXT PRIMARY KEY,
	last_used_at TEXT NOT NULL
);
"#;

pub fn apply(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(SCHEMA)?;
    reconcile_additive_columns(conn)?;
    conn.pragma_update(None, "user_version", crate::versioning::SCHEMA_VERSION)?;
    Ok(())
}

struct ColumnShape {
    name: String,
    decl_type: String,
    not_null: bool,
    default_value: Option<String>,
}

fn canonical_table_names(canonical: &Connection) -> Result<Vec<String>, Error> {
    let mut stmt = canonical.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<ColumnShape>, Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| {
        Ok(ColumnShape {
            name: row.get(1)?,
            decl_type: row.get(2)?,
            not_null: row.get::<_, i64>(3)? != 0,
            default_value: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

struct ForeignKeyShape {
    ref_table: String,
    ref_column: Option<String>,
    on_delete: String,
    on_update: String,
}

fn foreign_key_for_column(
    canonical: &Connection,
    table: &str,
    column_name: &str,
) -> Result<Option<ForeignKeyShape>, Error> {
    let mut stmt = canonical.prepare(&format!("PRAGMA foreign_key_list({table})"))?;
    let mut by_group: std::collections::HashMap<i64, Vec<ForeignKeyShape>> =
        std::collections::HashMap::new();
    let rows = stmt.query_map([], |row| {
        let group_id: i64 = row.get(0)?;
        let ref_table: String = row.get(2)?;
        let from_column: String = row.get(3)?;
        let ref_column: Option<String> = row.get(4)?;
        let on_update: String = row.get(5)?;
        let on_delete: String = row.get(6)?;
        Ok((
            group_id,
            from_column,
            ForeignKeyShape {
                ref_table,
                ref_column,
                on_delete,
                on_update,
            },
        ))
    })?;
    for row in rows {
        let (group_id, from_column, shape) = row?;
        if from_column == column_name {
            by_group.entry(group_id).or_default().push(shape);
        }
    }
    match by_group.into_values().next() {
        None => Ok(None),
        Some(mut group) if group.len() == 1 => Ok(Some(group.remove(0))),
        Some(_) => Err(Error::Invalid(format!(
            "{table}.{column_name} is missing from an existing file and its foreign key is composite; this is not an additive change, bump SCHEMA_VERSION instead"
        ))),
    }
}

fn table_create_sql(conn: &Connection, table: &str) -> Result<String, Error> {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get(0),
    )
    .map_err(Error::from)
}

fn strip_sql_line_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '-' && chars.peek() == Some(&'-') {
            for rest in chars.by_ref() {
                if rest == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn column_source_segment(create_sql: &str, column_name: &str) -> Option<String> {
    let create_sql = strip_sql_line_comments(create_sql);
    let open = create_sql.find('(')?;
    let close = create_sql.rfind(')')?;
    if close <= open {
        return None;
    }
    let body = &create_sql[open + 1..close];
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut segments = Vec::new();
    for (i, ch) in body.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                segments.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    segments.push(&body[start..]);
    segments
        .into_iter()
        .map(str::trim)
        .find(|segment| {
            segment
                .split(char::is_whitespace)
                .next()
                .is_some_and(|token| token.eq_ignore_ascii_case(column_name))
        })
        .map(str::to_string)
}

fn column_carries_an_unreproducible_constraint(segment: &str) -> bool {
    let upper = segment.to_ascii_uppercase();
    upper.contains("CHECK")
        || upper.contains("UNIQUE")
        || upper.contains("COLLATE")
        || upper.contains("GENERATED")
}

fn add_column(
    conn: &Connection,
    table: &str,
    column: &ColumnShape,
    foreign_key: Option<&ForeignKeyShape>,
) -> Result<(), Error> {
    let mut ddl = format!(
        "ALTER TABLE {table} ADD COLUMN {} {}",
        column.name, column.decl_type
    );
    if column.not_null {
        ddl.push_str(" NOT NULL");
    }
    if let Some(default_value) = &column.default_value {
        ddl.push_str(" DEFAULT ");
        ddl.push_str(default_value);
    }
    if let Some(fk) = foreign_key {
        ddl.push_str(" REFERENCES ");
        ddl.push_str(&fk.ref_table);
        if let Some(ref_column) = &fk.ref_column {
            ddl.push('(');
            ddl.push_str(ref_column);
            ddl.push(')');
        }
        if fk.on_delete != "NO ACTION" {
            ddl.push_str(" ON DELETE ");
            ddl.push_str(&fk.on_delete);
        }
        if fk.on_update != "NO ACTION" {
            ddl.push_str(" ON UPDATE ");
            ddl.push_str(&fk.on_update);
        }
    }
    conn.execute_batch(&ddl)?;
    Ok(())
}

fn reconcile_additive_columns(conn: &Connection) -> Result<(), Error> {
    let canonical = Connection::open_in_memory()?;
    canonical.execute_batch(SCHEMA)?;

    for table in canonical_table_names(&canonical)? {
        let existing: HashSet<String> = table_columns(conn, &table)?
            .into_iter()
            .map(|column| column.name)
            .collect();
        let create_sql = table_create_sql(&canonical, &table)?;

        for column in table_columns(&canonical, &table)? {
            if existing.contains(&column.name) {
                continue;
            }
            if column.not_null && column.default_value.is_none() {
                return Err(Error::Invalid(format!(
                    "{table}.{} is a NOT NULL column with no default missing from an existing file; this is not an additive change, bump SCHEMA_VERSION instead",
                    column.name
                )));
            }
            let source_segment =
                column_source_segment(&create_sql, &column.name).ok_or_else(|| {
                    Error::Invalid(format!(
                        "{table}.{} is missing from an existing file and its own definition could not be located in the canonical schema text; this is not a safe additive change, bump SCHEMA_VERSION instead",
                        column.name
                    ))
                })?;
            if column_carries_an_unreproducible_constraint(&source_segment) {
                return Err(Error::Invalid(format!(
                    "{table}.{} is missing from an existing file and carries a constraint an ALTER TABLE ADD COLUMN cannot faithfully reproduce; this is not an additive change, bump SCHEMA_VERSION instead",
                    column.name
                )));
            }
            let foreign_key = foreign_key_for_column(&canonical, &table, &column.name)?;
            add_column(conn, &table, &column, foreign_key.as_ref())?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::confirmation::Confirmation;
    use crate::ids::{DocId, SessionId};
    use crate::obs_origin::ObsOrigin;
    use crate::observation::{self, ObservationMeta, StatFacts};

    /// Seeds one `documents` row, one `sessions` row, and one `blobs` row —
    /// the minimum an `observations` row's FKs need.
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
        apply(&conn).expect("apply");
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
        apply(&conn).expect("apply");
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
        let conn = Connection::open_in_memory().expect("open");
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

        apply(&conn).expect("apply reconciles the missing column");

        let read_confirmed = |conn: &Connection| -> Option<i64> {
            conn.query_row(
                "SELECT confirmed FROM observations WHERE doc_id = ?1",
                [doc_id],
                |row| row.get(0),
            )
            .expect("read back confirmed")
        };
        assert_eq!(read_confirmed(&conn), None);

        apply(&conn).expect("a second apply against an already-reconciled file is a no-op");
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
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(OBSERVATIONS_MISSING_REQUIRED_ORIGIN_COLUMN)
            .expect("apply the shape missing a required column");

        let err =
            apply(&conn).expect_err("a NOT NULL column with no default is not an additive change");
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
        let conn = Connection::open_in_memory().expect("open");
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

        apply(&conn).expect("apply adds parent_a with its foreign key intact");

        let result = conn.execute(
            "INSERT INTO observations(doc_id, session_id, blob_hash, origin, at, parent_a) VALUES (?1, ?2, 'h', 'probe', 'x', 999999)",
            rusqlite::params![doc_id, session_id],
        );
        assert!(
            result.is_err(),
            "a parent_a value with no matching observations row must be rejected by the foreign key the reconciled column carries, not silently accepted"
        );
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
        let create_sql =
            table_create_sql(&canonical, "observations").expect("read real create sql");

        let segment = column_source_segment(&create_sql, "parent_a")
            .expect("a column's own definition must be found in the schema this crate ships");

        assert_eq!(segment, "parent_a INTEGER REFERENCES observations(id)");
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
}
