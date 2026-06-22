package docstate

import (
	"database/sql"
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"

	_ "github.com/mattn/go-sqlite3"

	"rune/pkg/vfs"
)

// Store holds the SQLite connection used for all persistence.
//
// perm is the on-disk database (rune.db) that survives process restarts; it
// also holds the event journal. There is no separate in-memory DB — events
// and snapshots live in the same durable store. For ephemeral runs (fuzz,
// OpenInMemory) perm itself is an :memory: database.
//
// SetMaxOpenConns(1) is called on every handle so that:
//   - multi-statement transactions issued via db.Begin() all land on the same
//     connection (otherwise BEGIN and COMMIT could reach different pool slots
//     and run in autocommit, silently defeating atomicity);
//   - :memory: stores always refer to the same anonymous database (each
//     connection to ":memory:" opens a distinct empty database).
type Store struct {
	perm  *sql.DB
	clock func() time.Time
	// fs supplies the file identity stat for OpenPath/Bind. A nil fs defaults to
	// vfs.Disk (real disk); the fuzz harness injects vfs.Mem via UseFS so the
	// store and workspace resolve identity against the same in-memory files.
	fs vfs.FS
}

// UseFS injects the filesystem used for file-identity stats (OpenPath/Bind).
// Production leaves it as the default Disk; the session fuzzer sets a shared
// vfs.Mem so document identity matches the in-memory files the workspace writes.
func (s *Store) UseFS(fs vfs.FS) { s.fs = fs }

// statID returns the (inode, device) identity of path via the injected FS,
// defaulting to real disk. ok is false when stat fails or identity is
// unavailable, in which case the caller degrades to path-keying.
func (s *Store) statID(path string) (inode, device uint64, ok bool) {
	fsys := s.fs
	if fsys == nil {
		fsys = vfs.Disk{}
	}
	fi, err := fsys.Stat(path)
	if err != nil {
		return 0, 0, false
	}
	return vfs.FileID(fi)
}

// DocRef is returned by OpenPath and CreateScratch.
// ID is the stable VFS document primary key.
// RenamedFrom is set when OpenPath detects that the file was renamed since the
// VFS last saw it.
type DocRef struct {
	ID          int64
	RenamedFrom string // non-empty when a rename was detected
}

// ---- schema -----------------------------------------------------------------

// permSchema is the canonical, complete schema for a fresh database.
// It includes all tables, CHECK/FK constraints, and UNIQUE indexes.
// migrate() v3 drops legacy data tables and re-runs this schema so that
// existing installations converge to the same final shape.
//
// current_seq is a position (seq-1 after an undo), not a foreign key —
// it need not match an existing event row. This is a conscious denormalization.
const permSchema = `
CREATE TABLE IF NOT EXISTS documents (
	id           INTEGER PRIMARY KEY,
	path         TEXT    NOT NULL DEFAULT '',
	inode        INTEGER,
	device       INTEGER,
	current_seq  INTEGER CHECK(current_seq IS NULL OR current_seq >= 0),
	saved_seq    INTEGER CHECK(saved_seq IS NULL OR saved_seq >= 0),
	created_at   TEXT    NOT NULL,
	last_seen_at TEXT    NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_inode ON documents(inode, device) WHERE inode IS NOT NULL AND inode != 0;
CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_path  ON documents(path)           WHERE path != '';

CREATE TABLE IF NOT EXISTS blobs (
	hash    TEXT PRIMARY KEY,
	content BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS snapshots (
	id         INTEGER PRIMARY KEY,
	doc_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
	blob_hash  TEXT    NOT NULL REFERENCES blobs(hash),
	source     TEXT    NOT NULL,
	seq        INTEGER NOT NULL DEFAULT 0 CHECK(seq >= 0),
	created_at TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snapshots_doc ON snapshots(doc_id, id);

CREATE TABLE IF NOT EXISTS events (
	seq                INTEGER PRIMARY KEY AUTOINCREMENT,
	doc_id             INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
	surface            TEXT    NOT NULL,
	kind               TEXT    NOT NULL CHECK(kind <> ''),
	edits              BLOB,
	cursors_before     BLOB,
	cursors_after      BLOB,
	focus_before       TEXT,
	focus_after        TEXT,
	is_undo_stop       INTEGER NOT NULL DEFAULT 0 CHECK(is_undo_stop IN (0,1)),
	anchor_snapshot_id INTEGER,
	at                 TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_doc  ON events(doc_id, seq);
CREATE INDEX IF NOT EXISTS idx_events_undo ON events(doc_id, seq) WHERE is_undo_stop = 1;

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

func openPerm(path string) (*sql.DB, error) {
	dsn := fmt.Sprintf("file:%s?_foreign_keys=on&_txlock=immediate&_busy_timeout=5000", path)
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
	if err := migrate(db); err != nil {
		return fmt.Errorf("migrate schema: %w", err)
	}
	return nil
}

// Open opens (or creates) the on-disk rune.db and starts the VFS store.
//
// The returned string is a non-fatal degradation warning; callers may surface
// it to the user but MUST NOT fail on a non-empty warning. The error return is
// reserved for hard failures (e.g., :memory: itself cannot be opened).
//
// Open ladder:
//  1. Try $HOME/.local/share/rune/rune.db
//  2. On failure: os.MkdirAll and retry
//  3. On failure: fall back to :memory: for perm + set warning
//  4. Hard fail only if :memory: fails
func Open() (*Store, string, error) {
	permPath := filepath.Join(os.Getenv("HOME"), ".local", "share", "rune", "rune.db")

	perm, permErr := openPerm(permPath)
	if permErr != nil {
		if mkErr := os.MkdirAll(filepath.Dir(permPath), 0o700); mkErr == nil {
			perm, permErr = openPerm(permPath)
		}
	}

	var warning string
	if permErr != nil {
		var err error
		perm, err = openInMemPerm()
		if err != nil {
			return nil, "", fmt.Errorf("open fallback perm db: %w", err)
		}
		warning = "history disabled — storage unavailable"
	}

	return &Store{perm: perm, clock: time.Now}, warning, nil
}

// OpenInMemory creates a Store with the perm database in :memory:.
// Useful for testing and fuzzing — no disk I/O, no path threading required.
func OpenInMemory(clock func() time.Time) (*Store, error) {
	perm, err := openInMemPerm()
	if err != nil {
		return nil, err
	}
	if clock == nil {
		clock = time.Now
	}
	return &Store{perm: perm, clock: clock}, nil
}

// OpenAt creates a Store backed by baseDir/rune.db.
// Intended for tests that need real files (durability, crash-recovery).
func OpenAt(baseDir string) (*Store, string, error) {
	permPath := filepath.Join(baseDir, "rune.db")

	perm, permErr := openPerm(permPath)
	if permErr != nil {
		if mkErr := os.MkdirAll(baseDir, 0o700); mkErr == nil {
			perm, permErr = openPerm(permPath)
		}
	}

	var warning string
	if permErr != nil {
		var err error
		perm, err = openInMemPerm()
		if err != nil {
			return nil, "", fmt.Errorf("open fallback perm db: %w", err)
		}
		warning = "history disabled — storage unavailable"
	}

	return &Store{perm: perm, clock: time.Now}, warning, nil
}

// SetClock replaces the Store's clock function. Used in deterministic tests.
func (s *Store) SetClock(clock func() time.Time) {
	s.clock = clock
}

// NewTestStore opens a Store suitable for tests: perm backed by a temp file.
// The store is closed automatically via t.Cleanup.
func NewTestStore(t *testing.T) *Store {
	t.Helper()
	permPath := filepath.Join(t.TempDir(), "rune_test.db")

	perm, err := openPerm(permPath)
	if err != nil {
		t.Fatalf("NewTestStore: open perm: %v", err)
	}

	s := &Store{perm: perm, clock: time.Now}
	t.Cleanup(func() {
		if err := s.Close(); err != nil {
			t.Logf("NewTestStore cleanup: %v", err)
		}
	})
	return s
}

// Close closes the underlying database connection.
func (s *Store) Close() error {
	if s.perm != nil {
		if err := s.perm.Close(); err != nil {
			return fmt.Errorf("close perm db: %w", err)
		}
	}
	return nil
}

// ---- migration --------------------------------------------------------------

// migrate converges any existing database to the current schema.
//
// v3: drop all data tables (owner authorised the drop), then re-run permSchema
// so the constrained, fully-indexed schema is in place. A fresh DB (version 0)
// runs permSchema directly from initPermSchema and the v3 drop-if-exists is a
// harmless no-op. drafts/search_history have no FK children and are preserved.
func migrate(db *sql.DB) error {
	var version int
	if err := db.QueryRow(`PRAGMA user_version`).Scan(&version); err != nil {
		return fmt.Errorf("get user_version: %w", err)
	}
	if version < 3 {
		// Drop in FK-safe child→parent order so FK enforcement cannot block the drops.
		for _, stmt := range []string{
			`DROP TABLE IF EXISTS events`,
			`DROP TABLE IF EXISTS snapshots`,
			`DROP TABLE IF EXISTS blobs`,
			`DROP TABLE IF EXISTS documents`,
		} {
			if _, err := db.Exec(stmt); err != nil {
				return fmt.Errorf("migrate v3: %s: %w", stmt, err)
			}
		}
		if _, err := db.Exec(permSchema); err != nil {
			return fmt.Errorf("migrate v3: recreate schema: %w", err)
		}
		if _, err := db.Exec(`PRAGMA user_version = 3`); err != nil {
			return fmt.Errorf("migrate v3: set user_version: %w", err)
		}
	}
	return nil
}

// ---- documents --------------------------------------------------------------

// OpenPath resolves the VFS document for a file that exists on disk.
// It must only be called after the file has been successfully read (so stat
// can obtain a real inode). Returns a DocRef with the stable document ID;
// RenamedFrom is set when the file was renamed since the VFS last saw it.
func (s *Store) OpenPath(path string) (DocRef, error) {
	at := s.clock().UTC().Format(time.RFC3339Nano)
	inode, device, ok := s.statID(path)

	if !ok || inode == 0 {
		return s.openPathByName(path, at)
	}
	return s.openPathByInode(path, inode, device, at)
}

// openPathByName is the path-keying fallback used when inode is unavailable.
func (s *Store) openPathByName(path, at string) (DocRef, error) {
	var id int64
	err := s.perm.QueryRow(
		`SELECT id FROM documents WHERE path=? AND (inode IS NULL OR inode=0)`,
		path,
	).Scan(&id)
	if err == sql.ErrNoRows {
		res, err := s.perm.Exec(
			`INSERT INTO documents(path, inode, device, created_at, last_seen_at) VALUES(?,0,0,?,?)`,
			path, at, at,
		)
		if err != nil {
			return DocRef{}, fmt.Errorf("open path %q: insert: %w", path, err)
		}
		id, err = res.LastInsertId()
		if err != nil {
			return DocRef{}, fmt.Errorf("open path %q: last insert id: %w", path, err)
		}
		return DocRef{ID: id}, nil
	}
	if err != nil {
		return DocRef{}, fmt.Errorf("open path %q: query: %w", path, err)
	}
	if _, err := s.perm.Exec(`UPDATE documents SET last_seen_at=? WHERE id=?`, at, id); err != nil {
		return DocRef{}, fmt.Errorf("open path %q: update last_seen_at: %w", path, err)
	}
	return DocRef{ID: id}, nil
}

func (s *Store) openPathByInode(path string, inode, device uint64, at string) (DocRef, error) {
	var rowID int64
	var rowPath string
	err := s.perm.QueryRow(
		`SELECT id, path FROM documents WHERE inode=? AND device=?`,
		inode, device,
	).Scan(&rowID, &rowPath)

	if err == sql.ErrNoRows {
		// New inode: evict any stale path holder and insert fresh row.
		tx, txErr := s.perm.Begin()
		if txErr != nil {
			return DocRef{}, fmt.Errorf("open path %q: begin tx: %w", path, txErr)
		}
		if _, execErr := tx.Exec(
			`UPDATE documents SET path='' WHERE path=? AND (inode IS NULL OR inode!=?)`,
			path, inode,
		); execErr != nil {
			tx.Rollback() //nolint:errcheck
			return DocRef{}, fmt.Errorf("open path %q: evict stale holder: %w", path, execErr)
		}
		res, execErr := tx.Exec(
			`INSERT INTO documents(path, inode, device, created_at, last_seen_at) VALUES(?,?,?,?,?)`,
			path, inode, device, at, at,
		)
		if execErr != nil {
			tx.Rollback() //nolint:errcheck
			return DocRef{}, fmt.Errorf("open path %q: insert: %w", path, execErr)
		}
		newID, execErr := res.LastInsertId()
		if execErr != nil {
			tx.Rollback() //nolint:errcheck
			return DocRef{}, fmt.Errorf("open path %q: last insert id: %w", path, execErr)
		}
		if commitErr := tx.Commit(); commitErr != nil {
			return DocRef{}, fmt.Errorf("open path %q: commit: %w", path, commitErr)
		}
		return DocRef{ID: newID}, nil
	}
	if err != nil {
		return DocRef{}, fmt.Errorf("open path %q: query by inode: %w", path, err)
	}

	// Found by inode.
	var renamedFrom string
	if rowPath != path {
		renamedFrom = rowPath
		tx, txErr := s.perm.Begin()
		if txErr != nil {
			return DocRef{}, fmt.Errorf("open path %q: begin rename tx: %w", path, txErr)
		}
		// Free any other row that claims the new path.
		if _, execErr := tx.Exec(
			`UPDATE documents SET path='' WHERE path=? AND id!=?`,
			path, rowID,
		); execErr != nil {
			tx.Rollback() //nolint:errcheck
			return DocRef{}, fmt.Errorf("open path %q: free old holder: %w", path, execErr)
		}
		if _, execErr := tx.Exec(
			`UPDATE documents SET path=?, inode=?, device=?, last_seen_at=? WHERE id=?`,
			path, inode, device, at, rowID,
		); execErr != nil {
			tx.Rollback() //nolint:errcheck
			return DocRef{}, fmt.Errorf("open path %q: rebind rename: %w", path, execErr)
		}
		if commitErr := tx.Commit(); commitErr != nil {
			return DocRef{}, fmt.Errorf("open path %q: commit rename: %w", path, commitErr)
		}
	} else {
		if _, err := s.perm.Exec(`UPDATE documents SET last_seen_at=? WHERE id=?`, at, rowID); err != nil {
			return DocRef{}, fmt.Errorf("open path %q: update last_seen_at: %w", path, err)
		}
	}
	return DocRef{ID: rowID, RenamedFrom: renamedFrom}, nil
}

// CreateScratch inserts a new unbound (untitled) VFS document and returns its
// DocRef. The display name is managed by the caller (workspace title component).
func (s *Store) CreateScratch(_ string) (DocRef, error) {
	at := s.clock().UTC().Format(time.RFC3339Nano)
	res, err := s.perm.Exec(
		`INSERT INTO documents(path, inode, device, created_at, last_seen_at) VALUES('',0,0,?,?)`,
		at, at,
	)
	if err != nil {
		return DocRef{}, fmt.Errorf("create scratch: insert: %w", err)
	}
	id, err := res.LastInsertId()
	if err != nil {
		return DocRef{}, fmt.Errorf("create scratch: last insert id: %w", err)
	}
	return DocRef{ID: id}, nil
}

// ReserveChatDoc returns the stable ID of the per-session chat document.
// It uses a sentinel path ("\x00chat") that can never name a real file.
// Events from the previous session are truncated so each launch starts clean.
func (s *Store) ReserveChatDoc() (int64, error) {
	const sentinel = "\x00chat"
	at := s.clock().UTC().Format(time.RFC3339Nano)

	tx, err := s.perm.Begin()
	if err != nil {
		return 0, fmt.Errorf("reserve chat doc: begin: %w", err)
	}
	if _, err := tx.Exec(
		`INSERT OR IGNORE INTO documents(path, inode, device, created_at, last_seen_at) VALUES(?,0,0,?,?)`,
		sentinel, at, at,
	); err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("reserve chat doc: insert sentinel: %w", err)
	}
	var id int64
	if err := tx.QueryRow(`SELECT id FROM documents WHERE path=?`, sentinel).Scan(&id); err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("reserve chat doc: select id: %w", err)
	}
	if _, err := tx.Exec(`DELETE FROM events WHERE doc_id=?`, id); err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("reserve chat doc: truncate events: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return 0, fmt.Errorf("reserve chat doc: commit: %w", err)
	}
	return id, nil
}

// Bind (re)binds document docID to path, adopting the file's CURRENT inode/
// device and preserving the document id (and thus its full undo history). It is
// called in two places:
//
//   - first materialize of an untitled doc (naming / first save) — adopts the
//     freshly created file's inode;
//   - after EVERY overwrite save — the atomic write (temp→rename) gives the file
//     a NEW inode, so the recorded inode goes stale. Without re-binding, the next
//     OpenPath sees a "new inode at this path", evicts this row to path=” and
//     creates a fresh history-less doc — orphaning the undo DAG (§1.4.6) and
//     leaving a zombie row. Re-binding on save keeps identity stable across the
//     inode churn.
//
// Conflicting holders of the path or the new inode are evicted first so the
// unique indexes hold.
func (s *Store) Bind(docID int64, path string) error {
	at := s.clock().UTC().Format(time.RFC3339Nano)
	inode, device, ok := s.statID(path)

	tx, err := s.perm.Begin()
	if err != nil {
		return fmt.Errorf("bind %d → %q: begin: %w", docID, path, err)
	}
	// Free any other row holding this path (stale binding).
	if _, err := tx.Exec(`UPDATE documents SET path='' WHERE path=? AND id!=?`, path, docID); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("bind %d: free path holder: %w", docID, err)
	}
	if ok && inode != 0 {
		// Evict any stale row claiming this inode (deleted+recreated, or our own
		// prior inode reused by the filesystem).
		if _, err := tx.Exec(
			`UPDATE documents SET inode=0, device=0 WHERE inode=? AND device=? AND id!=?`,
			inode, device, docID,
		); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("bind %d: evict inode holder: %w", docID, err)
		}
		if _, err := tx.Exec(
			`UPDATE documents SET path=?, inode=?, device=?, last_seen_at=? WHERE id=?`,
			path, inode, device, at, docID,
		); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("bind %d: rebind: %w", docID, err)
		}
	} else {
		if _, err := tx.Exec(
			`UPDATE documents SET path=?, last_seen_at=? WHERE id=?`,
			path, at, docID,
		); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("bind %d: rebind by path: %w", docID, err)
		}
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("bind %d: commit: %w", docID, err)
	}
	return nil
}

// DeleteDoc removes a document and its journal/snapshots from the VFS. Used
// when the user explicitly discards an untitled buffer so it is not offered for
// recovery on the next launch. ON DELETE CASCADE removes the child events and
// snapshots rows automatically. Orphaned blobs are left for a future blob GC.
func (s *Store) DeleteDoc(docID int64) error {
	if _, err := s.perm.Exec(`DELETE FROM documents WHERE id=?`, docID); err != nil {
		return fmt.Errorf("delete doc %d: %w", docID, err)
	}
	return nil
}

// GCEmptyScratch deletes unbound (untitled) documents that carry neither events
// nor snapshots — empty scratch rows left over from prior sessions. keepID is
// never deleted (the live untitled buffer). Returns the number of rows removed.
// The chat sentinel has a non-empty path, so it is never affected.
func (s *Store) GCEmptyScratch(keepID int64) (int64, error) {
	res, err := s.perm.Exec(`
		DELETE FROM documents
		WHERE path='' AND id!=?
		  AND id NOT IN (SELECT DISTINCT doc_id FROM events)
		  AND id NOT IN (SELECT DISTINCT doc_id FROM snapshots)`,
		keepID,
	)
	if err != nil {
		return 0, fmt.Errorf("gc empty scratch: %w", err)
	}
	n, _ := res.RowsAffected() // best-effort count; deletion already committed
	return n, nil
}

// RecoverableScratch returns the IDs of GENUINE untitled scratch documents that
// carry history (events or snapshots) from a prior session, excluding excludeID
// and the chat sentinel (non-empty path). Newest first. These rows hold unsaved
// work the user can recover on the next launch.
//
// The `inode = 0` filter is load-bearing: a genuine scratch always has inode 0
// (CreateScratch inserts inode=0), whereas an orphaned BOUND document whose path
// was cleared by inode-change eviction RETAINS its real inode. Without this
// filter those zombie rows surface as fake "Untitled" tabs showing real-file
// content (a data-corruption-looking bug). Emptiness is filtered by the caller,
// which reconstructs each candidate and drops empty/whitespace-only content.
func (s *Store) RecoverableScratch(excludeID int64) ([]int64, error) {
	rows, err := s.perm.Query(`
		SELECT id FROM documents
		WHERE path='' AND id!=? AND (inode IS NULL OR inode = 0)
		  AND (id IN (SELECT DISTINCT doc_id FROM events)
		    OR id IN (SELECT DISTINCT doc_id FROM snapshots))
		ORDER BY id DESC`,
		excludeID,
	)
	if err != nil {
		return nil, fmt.Errorf("recoverable scratch: %w", err)
	}
	defer rows.Close()

	var ids []int64
	for rows.Next() {
		var id int64
		if err := rows.Scan(&id); err != nil {
			return nil, fmt.Errorf("recoverable scratch: scan: %w", err)
		}
		ids = append(ids, id)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("recoverable scratch: rows: %w", err)
	}
	return ids, nil
}
