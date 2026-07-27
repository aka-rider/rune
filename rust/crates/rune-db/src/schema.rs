//! The `rune-db` schema: a port of Go's `permSchema` v10
//! (`pkg/docstate/store_schema.go:122-264`), **minus `drafts`** (plan
//! decision 13 — the Rust port has no chat/drafts consumer yet; porting the
//! table now would lock in a scoping decision that belongs to that future
//! feature instead).
//!
//! Unlike Go's `dropIfStale` migration policy (schema-shape changes drop and
//! recreate the whole file), this crate versions by **filename**
//! (`versioning.rs`, plan decision 2): a schema-shape change ships as a new
//! `rune-v{N}.db`, so `SCHEMA` here only ever needs to describe a single,
//! frozen shape applied once to a brand-new file. `PRAGMA user_version` is
//! still stamped as a sanity check, but the filename — not this pragma — is
//! the real version (`versioning.rs`).
//!
//! Table-by-table rationale (inode/device NULL-not-zero, `observations`'s
//! deliberately cascade-free session FK, `session_documents` holding
//! per-session undo position and CAS baseline, etc.) is preserved verbatim
//! from the Go source as inline comments below — this is a faithful
//! transcription, not a redesign.

use rusqlite::Connection;

use crate::Error;

/// The canonical, complete schema for a fresh database. Applied once, in a
/// single batch, to either a brand-new file or a freshly-created in-memory
/// database — this crate never patches a partial/legacy shape in place
/// (there is no migration path; see the module doc).
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
-- (v10, CONSTITUTION.md §12). proc_started_at is the OS-reported start time
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
	-- supersedes: the saved_obs this row's adoption REPLACED (NULL if there
	-- was none) — recorded by every adoption primitive in the SAME tx as the
	-- saved_obs move, so a resolve-abandon can restore the exact prior
	-- baseline later.
	supersedes INTEGER REFERENCES observations(id),
	at        TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_observations_doc ON observations(doc_id, id);

CREATE TABLE IF NOT EXISTS search_history (
	query        TEXT PRIMARY KEY,
	last_used_at TEXT NOT NULL
);
"#;

/// The `PRAGMA user_version` sanity stamp. Not the real version (the
/// filename is, per `versioning.rs`) — a defensive marker so a future reader
/// that opens a `rune-v{N}.db` file directly (e.g. `sqlite3` on the CLI) can
/// tell schema shape apart from an empty file at a glance.
pub const USER_VERSION_STAMP: u32 = 1;

/// Applies `SCHEMA` to `conn` and stamps `PRAGMA user_version`. Idempotent
/// (`CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS` throughout) —
/// safe to call on every open, not just first creation.
pub fn apply(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(SCHEMA)?;
    conn.pragma_update(None, "user_version", USER_VERSION_STAMP)?;
    Ok(())
}
