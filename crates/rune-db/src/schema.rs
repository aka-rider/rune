//! The `rune-db` schema, **minus `drafts`** (plan
//! decision 13 — there is no chat/drafts consumer yet; adding the
//! table now would lock in a scoping decision that belongs to that future
//! feature instead).
//!
//! This crate versions by **filename** (`versioning.rs`, plan decision 2)
//! rather than migrating a schema in place: a schema-shape change ships as
//! a new `rune-v{N}.db`, leaving the old file and its journal untouched, so
//! `SCHEMA` here only ever needs to describe a single, frozen shape applied
//! once to a brand-new file. `PRAGMA user_version` is still stamped as a
//! sanity check, but the filename — not this pragma — is the real version
//! (`versioning.rs`).
//!
//! One narrow exception to "frozen shape": a nullable column added to an
//! existing table, with no change to any other column or table, is safe to
//! land in place under the SAME filename — a concurrently running older
//! binary keeps working unmodified (its inserts simply omit the new column
//! and get `NULL`; its named-column reads never see it). That is not the
//! partial/legacy-shape hazard filename versioning exists to avoid, so it
//! never bumps [`crate::versioning::SCHEMA_VERSION`]. Anything else —
//! rewriting, renaming, retyping, or dropping an existing column or table,
//! or any change an old binary could observe as a broken assumption — still
//! requires a real version bump, and that bump must also work out what it
//! means for the old file: a concurrently running old binary going on using
//! it as normal, and the eventual GC (`versioning.rs`) that reclaims
//! abandoned old-version files once every session referencing them is dead.
//!
//! Table-by-table rationale (inode/device NULL-not-zero, `observations`'s
//! deliberately cascade-free session FK, `session_documents` holding
//! per-session undo position and CAS baseline, etc.) is documented inline
//! as comments below.

use rusqlite::Connection;

use crate::Error;

/// The canonical, complete schema for a fresh database. Applied once, in a
/// single batch, to either a brand-new file or a freshly-created in-memory
/// database — this crate never patches a partial/legacy shape in place
/// (there is no migration path; see the module doc). [`apply`] additionally
/// runs [`ensure_additive_columns`], which is the one place a nullable
/// column may be added to an already-existing table under this same,
/// unbumped version — see the module doc's carve-out.
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

-- sessions: one row per Store construction (one per rune process, in
-- production) — the process identity that lets the journal tell its own
-- history apart from a DIFFERENT process's edits sharing the same store
-- (v10). proc_started_at is the OS-reported start time
-- of pid, recorded once at construction (session.rs) — the only thing that
-- lets a LATER session tell "pid still running MY writer" apart from "pid
-- recycled to an unrelated process since". A session row is deliberately
-- NEVER deleted by the reaper (WP4) — only its session_documents/events/
-- snapshots footprint — so a dead session's own observations (see below)
-- always keep a valid FK target.
--
-- FROZEN LIVENESS CONTRACT (versioning.rs): every rune-v*.db, past and
-- future, must satisfy `SELECT pid, proc_started_at FROM sessions` — this
-- table and these two columns may gain siblings but must never be renamed
-- or retyped.
CREATE TABLE IF NOT EXISTS sessions (
	id              INTEGER PRIMARY KEY,
	pid             INTEGER NOT NULL,
	proc_started_at TEXT    NOT NULL,
	opened_at       TEXT    NOT NULL
);

-- session_documents: undo position (current_seq) and CAS baseline
-- (saved_obs) are inherently PER-SESSION once two sessions can independently
-- edit the same doc_id (v10): a document's undo/redo head and "what we last
-- wrote or adopted" are both facts belonging to the session that produced
-- them, never shared. documents itself keeps only identity fields
-- (path/inode/device/kind/timestamps).
CREATE TABLE IF NOT EXISTS session_documents (
	session_id  INTEGER NOT NULL REFERENCES sessions(id)  ON DELETE CASCADE,
	doc_id      INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
	current_seq INTEGER CHECK(current_seq IS NULL OR current_seq >= 0),
	saved_obs   INTEGER REFERENCES observations(id),
	PRIMARY KEY(session_id, doc_id)
);

-- snapshots: PURE recovery anchors for RecoverDocument's replay — the disk
-- fact and the 3-way-merge ancestor are both served entirely by
-- observations/saved_obs/ancestorAt (WP4), never a snapshot-carried source
-- taxonomy. session_id (v10): a snapshot anchors ONE session's own replay
-- window — two sessions editing the same doc_id keep entirely separate
-- anchor chains, so neither can ever anchor its reconstruction on the
-- other's content.
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

-- events: one document = one event stream. No surface dimension — title is
-- never journaled and chat journals to its own reserved document, so doc_id
-- alone is always both the journal key and the recovery/undo unit.
-- session_id (v10): the journal author — AppendEdit's redo-truncation,
-- 300ms coalescing, and undo/redo position all scope to (doc_id,
-- session_id) together, so a session's own undo/redo can never see,
-- coalesce with, or truncate a DIFFERENT session's edits to the same doc.
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

-- observations: every disk state ever seen, by any origin. Two DIFFERENT
-- facts are read from this table and must never be conflated:
-- session_documents.saved_obs (the CAS expectation for the next write —
-- moves only on our own Materialize or an explicit ResolveAdopt) and the
-- 3-way-merge ancestor (derived on the fly: the newest 'load'|'save'|
-- 'resolve' observation with seq <= the undo position — never a stored
-- pointer, so undoing past a merge/discard automatically re-exposes the
-- divergence). seq is nullable: NULL means this sighting is not correlated
-- to any journal position (e.g. a bare probe).
--
-- session_id (v10, NOT NULL) is WHO recorded this sighting — required so the
-- ancestor's ELIGIBILITY filter can be scoped to "my own prior agreement" (a
-- different session's save/load/resolve is exactly as seq-correlated and
-- origin-eligible but must never silently become MY ancestor). Reading
-- "theirs" (the newest observation) stays deliberately UNSCOPED by session —
-- any session's disk fact is everyone's disk fact; only ancestor
-- ELIGIBILITY is session-scoped. Unlike events/snapshots/session_documents
-- above, this FK has NO ON DELETE CASCADE: a dead session's own
-- save/load/resolve observation must remain visible as "theirs" to every
-- other, still-live session forever, so the dead-session reaper (WP4) never
-- deletes the sessions row itself, only its now-unreachable
-- session_documents/events/snapshots footprint (once superseded).
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
	-- supersedes: the one lineage edge a row may carry, always recorded in
	-- the SAME tx as the row itself. Two distinct producers feed it: every
	-- adoption primitive points it at the saved_obs baseline the adoption
	-- REPLACED (so a resolve-abandon can restore the exact prior baseline
	-- later); observe_from_stat_tx points a CONFIRMED fresh sighting at
	-- whatever observation was newest a moment before it, when the two
	-- hashes differ (a re-confirmation of unchanged content chains to
	-- nothing new). NULL means a legacy/root row with no known predecessor.
	supersedes INTEGER REFERENCES observations(id),
	at        TEXT    NOT NULL,
	-- confirmed: NULL means legacy or not yet classified, 1 means a
	-- confirmed sighting, 0 an unconfirmed one. Storage only here — which
	-- reads earn which value is a later work item; this column is the
	-- additive, same-filename exception the module doc describes.
	confirmed INTEGER
);
CREATE INDEX IF NOT EXISTS idx_observations_doc ON observations(doc_id, id);

CREATE TABLE IF NOT EXISTS search_history (
	query        TEXT PRIMARY KEY,
	last_used_at TEXT NOT NULL
);
"#;

/// Applies `SCHEMA` to `conn` and stamps `PRAGMA user_version` with
/// [`crate::versioning::SCHEMA_VERSION`] — the ONE constant this crate's
/// version number is. Not the real version (the filename is, per
/// `versioning.rs`) — a defensive marker so a future reader that opens a
/// `rune-v{N}.db` file directly (e.g. `sqlite3` on the CLI) can tell schema
/// shape apart from an empty file at a glance, and so the pragma can never
/// drift out of sync with the filename it's supposed to echo. Idempotent
/// (`CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS` throughout) —
/// safe to call on every open, not just first creation.
pub fn apply(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(SCHEMA)?;
    ensure_additive_columns(conn)?;
    conn.pragma_update(None, "user_version", crate::versioning::SCHEMA_VERSION)?;
    Ok(())
}

/// The ONE chokepoint for the module doc's additive-column carve-out: for
/// every `(table, column, column_ddl)` this crate has ever added after that
/// table's original shape, checks `pragma table_info` and issues an
/// `ALTER TABLE … ADD COLUMN` only when the column is actually absent. A
/// brand-new database already has the column from [`SCHEMA`] above, so this
/// is a no-op there; an existing database on disk gets it exactly once, the
/// next time any binary new enough to know about it calls [`apply`]. Each
/// add is its own single DDL statement — SQLite applies a single statement
/// atomically, so there is no partial-column state to guard against, and no
/// existing row's data is ever rewritten by a nullable column showing up.
fn ensure_additive_columns(conn: &Connection) -> Result<(), Error> {
    const ADDITIVE_COLUMNS: &[(&str, &str, &str)] = &[("observations", "confirmed", "INTEGER")];

    for (table, column, ddl_type) in ADDITIVE_COLUMNS {
        if !has_column(conn, table, column)? {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {ddl_type}"
            ))?;
        }
    }
    Ok(())
}

/// Whether `table` already has a column named `column`, via `pragma
/// table_info` — the only reliable way to ask SQLite this without risking an
/// error from a redundant `ALTER TABLE ADD COLUMN` (SQLite has no `ADD
/// COLUMN IF NOT EXISTS`).
fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::observation::{self, ObservationMeta, StatFacts};

    /// Mirrors this crate's `observations` shape (and its FK dependencies)
    /// exactly as it stood before the `confirmed` column existed — the
    /// fixture the upgrade tests below apply [`apply`] against, standing in
    /// for a real `rune-v1.db` file a still-running old binary wrote to.
    /// Deliberately NOT the full [`SCHEMA`]: `session_documents`/
    /// `snapshots`/`events`/`search_history` play no part in the additive-
    /// column upgrade under test, and `apply`'s `CREATE TABLE IF NOT EXISTS`
    /// creates them fresh regardless of whether this fixture does.
    const PRE_CHANGE_SCHEMA: &str = r#"
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
    supersedes INTEGER REFERENCES observations(id),
    at         TEXT    NOT NULL
);
"#;

    /// Seeds one `documents` row, one `sessions` row, and one `blobs` row —
    /// the minimum an `observations` row's FKs need, on either the current
    /// or the pre-change shape (identical on both).
    fn seed_minimal(conn: &Connection) -> (i64, i64, String) {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();
        let session_id =
            crate::session::establish_session(conn, SystemTime::now()).expect("seed session");
        let hash = crate::blob::put_blob(conn, b"content").expect("seed blob");
        (doc_id, session_id, hash)
    }

    #[test]
    fn fresh_database_has_the_confirmed_column() {
        let conn = Connection::open_in_memory().expect("open");
        apply(&conn).expect("apply");
        assert!(
            has_column(&conn, "observations", "confirmed").expect("check column"),
            "a fresh database must have the confirmed column from SCHEMA directly"
        );
    }

    #[test]
    fn apply_upgrades_a_pre_change_database_idempotently_with_data_intact() {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(PRE_CHANGE_SCHEMA)
            .expect("apply pre-change shape");
        assert!(
            !has_column(&conn, "observations", "confirmed").expect("check column"),
            "the fixture must not already have the column apply() is meant to add"
        );

        let (doc_id, session_id, hash) = seed_minimal(&conn);
        conn.execute(
            "INSERT INTO observations(doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, at) \
             VALUES(?1,?2,?3,NULL,1,'t',NULL,NULL,NULL,'load','t')",
            rusqlite::params![doc_id, session_id, hash],
        )
        .expect("seed pre-change observation row");
        let obs_id = conn.last_insert_rowid();

        apply(&conn).expect("apply must upgrade the pre-change shape");
        assert!(
            has_column(&conn, "observations", "confirmed").expect("check column"),
            "apply() must have added the confirmed column"
        );
        let read_hash: String = conn
            .query_row(
                "SELECT blob_hash FROM observations WHERE id=?1",
                rusqlite::params![obs_id],
                |r| r.get(0),
            )
            .expect("row must survive the upgrade untouched");
        assert_eq!(read_hash, hash, "existing row data must be unrewritten");

        // Idempotent: a second apply() on an already-upgraded database is a
        // no-op, not an error (there is no `ADD COLUMN IF NOT EXISTS`, so a
        // naive re-add would fail here if `has_column`'s guard were wrong).
        apply(&conn).expect("a second apply() must not error");
        let read_hash_again: String = conn
            .query_row(
                "SELECT blob_hash FROM observations WHERE id=?1",
                rusqlite::params![obs_id],
                |r| r.get(0),
            )
            .expect("row must still be present after the second apply()");
        assert_eq!(read_hash_again, hash, "data must still be intact");
    }

    #[test]
    fn a_raw_insert_omitting_confirmed_still_works_after_upgrade() {
        // Simulates an old binary that has never heard of the confirmed
        // column, inserting into an ALREADY-upgraded database — the
        // module doc's concurrent-old-binary coexistence claim.
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(PRE_CHANGE_SCHEMA)
            .expect("apply pre-change shape");
        let (doc_id, session_id, hash) = seed_minimal(&conn);
        apply(&conn).expect("apply must upgrade the pre-change shape");

        conn.execute(
            "INSERT INTO observations(doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, at) \
             VALUES(?1,?2,?3,NULL,1,'t',NULL,NULL,NULL,'probe','t')",
            rusqlite::params![doc_id, session_id, hash],
        )
        .expect("an old-shape INSERT naming only pre-change columns must still succeed");
        let obs_id = conn.last_insert_rowid();

        let confirmed: Option<bool> = conn
            .query_row(
                "SELECT confirmed FROM observations WHERE id=?1",
                rusqlite::params![obs_id],
                |r| r.get(0),
            )
            .expect("row must be readable");
        assert_eq!(
            confirmed, None,
            "a column the old-shape INSERT never named must default to NULL"
        );
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
                origin: "probe",
                confirmed: None,
            },
            &StatFacts {
                size: 1,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("record observation");
        let obs = observation::get_observation(&tx, obs_id).expect("read back");
        assert_eq!(obs.confirmed, None);
        tx.commit().expect("commit");
    }

    #[test]
    fn record_observation_with_confirmed_round_trips_both_values() {
        let mut conn = Connection::open_in_memory().expect("open");
        apply(&conn).expect("apply");
        let (doc_id, session_id, hash) = seed_minimal(&conn);

        let tx = conn.transaction().expect("tx");
        let stat = StatFacts {
            size: 1,
            mtime: "t".to_string(),
            ..Default::default()
        };

        let true_id = observation::record_observation(
            &tx,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: None,
                origin: "probe",
                confirmed: Some(true),
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
                origin: "probe",
                confirmed: Some(false),
            },
            &stat,
            "t",
        )
        .expect("record confirmed=false");

        assert_eq!(
            observation::get_observation(&tx, true_id)
                .expect("read true")
                .confirmed,
            Some(true)
        );
        assert_eq!(
            observation::get_observation(&tx, false_id)
                .expect("read false")
                .confirmed,
            Some(false)
        );
        tx.commit().expect("commit");
    }
}
