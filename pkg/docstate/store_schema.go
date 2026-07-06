package docstate

import (
	"database/sql"
	"errors"
	"fmt"
	"os"
	"strings"
)

// ---- schema -----------------------------------------------------------------

// schemaVersion is the current PRAGMA user_version. Every schema-shape change
// bumps it — drop-on-mismatch (dropIfStale) is the PERMANENT migration policy
// (user decision, Data-Integrity Model v4): SQLite is a shadow of the real
// data (the .md files), so full DB loss on a shape change is semi-tolerable
// and outweighs the burden of backward-compatible migrations.
//
// v4 (WP3): events drops surface/kind/is_undo_stop/anchor_snapshot_id/
// focus_* (I2 — one document, one event stream — the journal key and the
// recovery/undo unit are now the same); documents gains kind.
//
// v5 (WP4, additive): documents gains saved_obs (the CAS expectation for the
// next Materialize write — a fact distinct from the 3-way merge ancestor,
// which is derived from journal position via ancestorAt, never stored);
// observations records every disk state ever SEEN (load/save/probe/watch/
// resolve/swap), the physical record capture-before-discard depends on.
//
// v6 (WP5, the workspace cutover): documents drops its last-saved journal
// position column — dirty is now ancestor-derived (IsDirty ⟺
// Sync(docID).Kind != Clean), not a separate saved position; snapshots drops
// its `source` taxonomy column — snapshots
// are now PURE recovery anchors for RecoverDocument's replay, with the disk
// fact (CAS baseline) and the 3-way-merge ancestor both served entirely by
// observations/saved_obs/ancestorAt (Part III "two facts, two mechanisms").
// current_seq is DELIBERATELY kept under its pre-v4 name — Part III's
// target schema sketch calls it head_seq, but no work package's change list
// instructs the rename, and it is purely cosmetic (zero behavioral
// difference across ~15 call sites) — a documented deviation, not an
// oversight.
//
// v7 (data-integrity v4 remediation, WP-R1): observations gains supersedes
// (the saved_obs an adoption replaced, NULL if none). ResolveAbandon (the
// store half of an Esc-abort out of the merge resolver) needs to undo an
// adoption (commitSave/ResolveAdopt/AdoptEqual) EXACTLY, restoring the prior
// CAS baseline verbatim rather than re-deriving it by an origin scan — an
// intervening diverged 'load'/'probe' sighting recorded between the original
// adoption and the abandon would poison a "newest save/load/resolve" scan
// into picking the wrong baseline. See the Adoption Contract in
// observation.go's package doc comment.
//
// v8 (code review remediation, §1.7): documents.inode/device now record
// "identity unknown" as SQL NULL, never the literal 0 previously written by
// the path-fallback OpenPath insert, CreateScratch, ReserveChatDoc, and every
// eviction UPDATE (Bind, commitSave) — a magic-value sentinel sharing the
// same column as a real identity, the exact pattern §1.7 forbids. Every write
// site converted atomically with idx_documents_inode, whose WHERE clause
// drops the now-dead `AND inode != 0` (NULL is the only "absent" spelling
// left, so two identity-less docs — e.g. two scratch documents open at once
// — no longer risk colliding on it) and RecoverableScratch's zombie filter
// (`inode IS NULL` replaces `inode IS NULL OR inode = 0`).
//
// v9 (code review remediation, D13/§1.7): observations.inode/device/nlink —
// already NULLable columns since the original schema, but never actually
// written as NULL — now follow the SAME NULL-when-absent discipline v8 gave
// documents: recordObservation/recordAdoptionTx take sql.NullInt64 args
// built from vfs.FileID's/vfs.FileNLink's own out-of-band `ok`, never
// reconstituted from the value as `inode != 0` (D12 — the exact §1.7
// violation commitSave's documents.inode/device write had regressed to,
// fixed at the SAME stat call this bump covers). Observation.Inode/Device/
// NLink change from bare uint64/int to sql.NullInt64 so a copy-forward
// adoption (ResolveAdopt/AdoptEqual, which re-persists a PRIOR observation's
// stat facts under a new origin) preserves "identity was unknown" instead of
// re-writing it as a false real value of 0. Drop-on-mismatch (dropIfStale)
// makes this migration-free — no existing row is patched in place.
//
// v10 (two-editors-session-fix): gives every journaled edit a process
// identity, so two rune processes sharing one workDir/rune.db no longer
// silently corrupt each other's journal or overwrite each other's saves —
// the fix for the removal of the global flock in 316adb9 (docstate: per-
// workspace store, drop global flock for SQLite-native concurrency), which
// correctly narrowed an over-broad refusal but deleted the underlying
// protection instead of re-deriving it. A new `sessions` table holds one row
// per Store construction (one per rune process); `events`/`snapshots` gain a
// NOT NULL session_id (the journal author — AppendEdit's redo-truncation,
// 300ms coalescing, and undo/redo position all scope to (doc_id,
// session_id), so a session's own undo/redo can never see, coalesce with,
// or truncate a DIFFERENT session's edits to the same doc); `observations`
// gains a NOT NULL session_id too, but for a DIFFERENT reason — ancestor
// ELIGIBILITY (ancestorAt) is scoped to "my own prior agreement" (never a
// different session's save/load/resolve silently becoming my ancestor and
// letting a 3-way merge discard their change with zero conflict markers),
// while "theirs" (newestObservation) stays deliberately UNSCOPED — any
// session's disk fact is everyone's disk fact, exactly what makes
// cross-session reconciliation through the existing conflict-guard/3-way-
// merge machinery possible at all. `documents.current_seq`/`saved_obs` move
// to a new `session_documents(session_id, doc_id)` table — undo position and
// CAS baseline are inherently per-session once two sessions can
// independently edit the same docID; `documents` keeps only identity fields.
// `observations.session_id` deliberately has NO ON DELETE CASCADE (unlike
// the other three): a dead session's own save/load/resolve observation must
// remain visible as "theirs" to every other, still-live session forever, so
// the dead-session reaper (liveness.go) never deletes the `sessions` row
// itself — only its (now unreachable) session_documents/events/snapshots
// footprint, once superseded. Drop-on-mismatch (dropIfStale) makes this
// migration-free — no existing row is patched in place.
const schemaVersion = 10

// permSchema is the canonical, complete schema for a fresh database.
// It includes all tables, CHECK/FK constraints, and UNIQUE indexes.
// dropIfStale deletes any database below schemaVersion before this runs, so
// permSchema always applies to either a brand-new file or a freshly-emptied
// one — never patches a partial/legacy shape in place.
//
// current_seq is a position (seq-1 after an undo), not a foreign key —
// it need not match an existing event row. This is a conscious denormalization.
// session_documents.saved_obs and observations.doc_id reference each other's
// tables; SQLite resolves FK targets at DML time, not CREATE TABLE time, so
// forward references (e.g. session_documents, defined before observations)
// are legal — a newly-adopted document's saved_obs is NULL until its first
// observation exists.
const permSchema = `
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
-- history apart from a DIFFERENT process's edits sharing the same workDir
-- (v10, CONSTITUTION.md §12). proc_started_at is the OS-reported start time
-- of pid, recorded once at construction (liveness.go) — the only thing that
-- lets a LATER session tell "pid still running MY writer" apart from "pid
-- recycled to an unrelated process since". A session row is deliberately
-- NEVER deleted by the reaper (liveness.go) — only its session_documents/
-- events/snapshots footprint — so a dead session's own observations (see
-- below) always keep a valid FK target.
CREATE TABLE IF NOT EXISTS sessions (
	id              INTEGER PRIMARY KEY,
	pid             INTEGER NOT NULL,
	proc_started_at TEXT    NOT NULL,
	opened_at       TEXT    NOT NULL
);

-- session_documents: undo position (current_seq) and CAS baseline
-- (saved_obs) — formerly documents.current_seq/saved_obs — are inherently
-- PER-SESSION once two sessions can independently edit the same docID (v10):
-- a document's undo/redo head and "what we last wrote or adopted" are both
-- facts belonging to the session that produced them, never shared. documents
-- itself keeps only identity fields (path/inode/device/kind/timestamps).
CREATE TABLE IF NOT EXISTS session_documents (
	session_id  INTEGER NOT NULL REFERENCES sessions(id)  ON DELETE CASCADE,
	doc_id      INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
	current_seq INTEGER CHECK(current_seq IS NULL OR current_seq >= 0),
	saved_obs   INTEGER REFERENCES observations(id),
	PRIMARY KEY(session_id, doc_id)
);

-- snapshots: PURE recovery anchors for RecoverDocument's replay (Part III —
-- the source taxonomy is deleted; the disk fact and the 3-way-merge ancestor
-- are both served entirely by observations/saved_obs/ancestorAt now).
-- session_id (v10): a snapshot anchors ONE session's own replay window — two
-- sessions editing the same doc_id keep entirely separate anchor chains, so
-- neither can ever anchor its reconstruction on the other's content.
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

-- events: one document = one event stream (I2). No surface dimension — title
-- is never journaled (ephemeral rename input, finalized on Enter) and chat
-- journals to its own reserved document (chatDocID), so doc_id alone is
-- always both the journal key and the recovery/undo unit. kind/is_undo_stop/
-- anchor_snapshot_id/focus_before/focus_after are dropped: kind was always
-- 'edit', every row was already an undo stop, and the rest were write-only.
-- session_id (v10): the journal author — AppendEdit's redo-truncation, 300ms
-- coalescing, and undo/redo position all scope to (doc_id, session_id)
-- together, so a session's own undo/redo can never see, coalesce with, or
-- truncate a DIFFERENT session's edits to the same doc (the journal race the
-- deleted global flock used to prevent by construction).
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
-- facts are read from this table and must never be conflated (Part III):
-- session_documents.saved_obs (the CAS expectation for the next write — moves
-- only on our own Materialize or an explicit ResolveAdopt) and the 3-way-merge
-- ancestor (derived on the fly by ancestorAt: the newest 'load'|'save'|
-- 'resolve' observation with seq <= the undo position — never a stored
-- pointer, so undoing past a merge/discard automatically re-exposes the
-- divergence). seq is nullable (§1.7): NULL means this sighting is not
-- correlated to any journal position (e.g. a bare probe).
--
-- session_id (v10, NOT NULL) is WHO recorded this sighting — required so
-- ancestorAt's ELIGIBILITY filter can be scoped to "my own prior agreement"
-- (a different session's save/load/resolve is exactly as seq-correlated and
-- origin-eligible but must never silently become MY ancestor — B1). Reading
-- "theirs" (newestObservation) stays deliberately UNSCOPED by session — any
-- session's disk fact is everyone's disk fact; only ancestor ELIGIBILITY is
-- session-scoped. Unlike events/snapshots/session_documents above, this FK
-- has NO ON DELETE CASCADE: a dead session's own save/load/resolve
-- observation must remain visible as "theirs" to every other, still-live
-- session forever, so the dead-session reaper (liveness.go) never deletes
-- the sessions row itself, only its now-unreachable session_documents/
-- events/snapshots footprint (once superseded).
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
	-- supersedes: the saved_obs this row's adoption REPLACED (NULL if there was
	-- none) — recorded by every adoption primitive (commitSave, ResolveAdopt,
	-- AdoptEqual) in the SAME tx as the saved_obs move, so ResolveAbandon can
	-- restore the exact prior baseline later (v7, WP-R1).
	supersedes INTEGER REFERENCES observations(id),
	at        TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_observations_doc ON observations(doc_id, id);

CREATE TABLE IF NOT EXISTS drafts (
	surface    TEXT PRIMARY KEY,
	content    TEXT NOT NULL,
	updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS search_history (
	query        TEXT PRIMARY KEY,
	last_used_at TEXT NOT NULL
);
`

// ---- construction -----------------------------------------------------------

// sqliteURIPath percent-encodes the three characters SQLite's own "file:"
// URI-filename parser (sqlite3ParseUri, driven here because mattn/go-sqlite3
// forwards a "file:"-prefixed DSN to sqlite3_open_v2 with SQLITE_OPEN_URI
// set — verified against the vendored sqlite3-binding.c) treats as
// syntactically significant when scanning the path component:
//
//   - '%' — the escape introducer; a literal '%' would have its next two
//     bytes decoded as a hex octet, silently mangling the path.
//   - '?' — the first UNESCAPED '?' ends the path and starts query-parameter
//     parsing; a workDir containing one would truncate the path and feed the
//     remainder in as bogus parameters ahead of our own
//     _foreign_keys/_txlock/_busy_timeout suffix.
//   - '#' — terminates the URI outright (everything after it, INCLUDING our
//     own appended query parameters, is silently dropped).
//
// No other byte (including '/', '&', whitespace) is special to that parser,
// so this is deliberately narrower than a general percent-encoder — it must
// never touch '/' (the path separator) or callers' own '?'-prefixed query
// suffix, which is appended AFTER this function runs. (S3)
func sqliteURIPath(path string) string {
	var b strings.Builder
	for i := 0; i < len(path); i++ {
		switch c := path[i]; c {
		case '%', '?', '#':
			fmt.Fprintf(&b, "%%%02X", c)
		default:
			b.WriteByte(c)
		}
	}
	return b.String()
}

// dropIfStale opens path just long enough to read PRAGMA user_version; a
// version below schemaVersion (0 = pre-v4, i.e. never versioned, or any
// earlier shape) deletes the database file and its -wal/-shm siblings so the
// caller's subsequent sql.Open starts from an empty file. A non-existent path
// is a no-op (the caller's open creates it fresh, already at version 0).
func dropIfStale(path string) error {
	if path == ":memory:" || path == "" {
		return nil // no file to version-check or drop
	}
	if _, err := os.Stat(path); errors.Is(err, os.ErrNotExist) {
		return nil
	}
	db, err := sql.Open("sqlite3", "file:"+sqliteURIPath(path))
	if err != nil {
		return fmt.Errorf("open for version check: %w", err)
	}
	version, verr := readUserVersion(db)
	closeErr := db.Close()
	if verr != nil {
		return verr
	}
	if closeErr != nil {
		return fmt.Errorf("close version-check handle: %w", closeErr)
	}
	if version >= schemaVersion {
		return nil
	}
	for _, suffix := range []string{"", "-wal", "-shm"} {
		if err := os.Remove(path + suffix); err != nil && !errors.Is(err, os.ErrNotExist) {
			return fmt.Errorf("drop-migration: remove %s%s: %w", path, suffix, err)
		}
	}
	return nil
}

func readUserVersion(db *sql.DB) (int, error) {
	var v int
	if err := db.QueryRow(`PRAGMA user_version`).Scan(&v); err != nil {
		return 0, fmt.Errorf("read user_version: %w", err)
	}
	return v, nil
}

func openPerm(path string) (*sql.DB, error) {
	if err := dropIfStale(path); err != nil {
		return nil, fmt.Errorf("check schema version %q: %w", path, err)
	}
	dsn := fmt.Sprintf("file:%s?_foreign_keys=on&_txlock=immediate&_busy_timeout=5000", sqliteURIPath(path))
	db, err := sql.Open("sqlite3", dsn)
	if err != nil {
		return nil, err
	}
	db.SetMaxOpenConns(1)
	if err := initPermSchema(db); err != nil {
		db.Close()
		return nil, err
	}
	return db, nil
}

func openInMemPerm() (*sql.DB, error) {
	dsn := "file::memory:?mode=memory&_foreign_keys=on&_txlock=immediate&_busy_timeout=5000"
	db, err := sql.Open("sqlite3", dsn)
	if err != nil {
		return nil, fmt.Errorf("open in-memory perm db: %w", err)
	}
	db.SetMaxOpenConns(1)
	if err := initPermSchema(db); err != nil {
		db.Close()
		return nil, fmt.Errorf("init in-memory perm schema: %w", err)
	}
	return db, nil
}

func initPermSchema(db *sql.DB) error {
	// WAL mode for file-backed databases; silent no-op for :memory:.
	if _, err := db.Exec(`PRAGMA journal_mode=WAL`); err != nil {
		return fmt.Errorf("set WAL mode: %w", err)
	}
	if _, err := db.Exec(`PRAGMA foreign_keys = ON`); err != nil {
		return fmt.Errorf("enable foreign keys: %w", err)
	}
	if _, err := db.Exec(permSchema); err != nil {
		return fmt.Errorf("init perm schema: %w", err)
	}
	if _, err := db.Exec(fmt.Sprintf(`PRAGMA user_version = %d`, schemaVersion)); err != nil {
		return fmt.Errorf("set user_version: %w", err)
	}
	return nil
}
