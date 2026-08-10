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
/// (there is no migration path; see the module doc). A nullable column added
/// to an already-existing table under this same, unbumped version — the
/// module doc's carve-out — lands directly in `SCHEMA` above; no additive
/// upgrade machinery is needed for a filename this crate itself has always
/// used unmodified.
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
-- recycled to an unrelated process since". The reaper deletes a dead
-- session's session_documents/events/snapshots footprint, then the row
-- itself too once it has recorded no observations (see below) — a row that
-- DID record one stays in place as that dead session's own permanent
-- "theirs" provenance, the only fact every other session may still need.
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
-- session_id (v10): the journal author — AppendEdit's redo-truncation and
-- undo/redo position both scope to (doc_id, session_id) together, so a
-- session's own undo/redo can never see or truncate a DIFFERENT session's
-- edits to the same doc.
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
	-- parent_a/parent_b: the version DAG's two lineage edges a row may
	-- carry, always recorded in the SAME tx as the row itself. parent_a is
	-- v1's single lineage edge, kept unrenamed in spirit: every adoption
	-- primitive points it at the saved_obs baseline the adoption REPLACED (so a
	-- resolve-abandon can restore the exact prior baseline later);
	-- observe_from_stat_tx points a CONFIRMED fresh sighting at whatever
	-- observation was newest a moment before it, when the two hashes differ
	-- (a re-confirmation of unchanged content chains to nothing new).
	-- parent_b is the second parent a two-parent join records: the disk-side
	-- observation a resolve/merge or a racing save reconciled against.
	-- NULL in either column means no such edge — a legacy/root row, or a
	-- one-parent join, carries no parent_b at all.
	parent_a INTEGER REFERENCES observations(id),
	parent_b INTEGER REFERENCES observations(id),
	at        TEXT    NOT NULL,
	-- confirmed: NULL means legacy or not yet classified, 1 means a
	-- confirmed sighting, 0 an unconfirmed one.
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
    conn.pragma_update(None, "user_version", crate::versioning::SCHEMA_VERSION)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::observation::{self, ObservationMeta, StatFacts};

    /// Seeds one `documents` row, one `sessions` row, and one `blobs` row —
    /// the minimum an `observations` row's FKs need.
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
                size: Some(1),
                mtime: Some("t".to_string()),
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
