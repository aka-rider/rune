package docstate

import (
	"database/sql"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"testing"
	"time"

	_ "github.com/mattn/go-sqlite3"
)

// Store holds the two SQLite connections used for persistence.
//
// perm is the on-disk database (rune.db) that survives process restarts.
// mem  is an in-process :memory: database used as the undo/redo journal.
//
// currentSeq tracks the undo pointer inside the journal:
//   - math.MaxInt64 means "at the end of history" (no undo has been applied).
//   - any other value N means we have undone to just before event N.
type Store struct {
	perm       *sql.DB
	mem        *sql.DB
	clock      func() time.Time
	currentSeq int64
}

// ---- schema -----------------------------------------------------------------

const permSchema = `
CREATE TABLE IF NOT EXISTS documents (
	id          INTEGER PRIMARY KEY,
	path        TEXT    NOT NULL DEFAULT '',
	inode       INTEGER,
	device      INTEGER,
	created_at  TEXT    NOT NULL,
	last_seen_at TEXT   NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_documents_path ON documents(path) WHERE path != '';

CREATE TABLE IF NOT EXISTS blobs (
	hash    TEXT PRIMARY KEY,
	content BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS snapshots (
	id         INTEGER PRIMARY KEY,
	doc_id     INTEGER NOT NULL REFERENCES documents(id),
	blob_hash  TEXT    NOT NULL REFERENCES blobs(hash),
	parent_ids TEXT,
	source     TEXT    NOT NULL,
	created_at TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snapshots_doc ON snapshots(doc_id, id);

CREATE TABLE IF NOT EXISTS drafts (
	surface    TEXT PRIMARY KEY,
	content    TEXT NOT NULL,
	updated_at TEXT NOT NULL
);
`

const memSchema = `
CREATE TABLE IF NOT EXISTS events (
	seq              INTEGER PRIMARY KEY AUTOINCREMENT,
	surface          TEXT    NOT NULL,
	kind             TEXT    NOT NULL,
	edits            BLOB,
	cursors_before   BLOB,
	cursors_after    BLOB,
	focus_before     TEXT,
	focus_after      TEXT,
	is_undo_stop     INTEGER NOT NULL DEFAULT 0,
	anchor_snapshot_id INTEGER,
	at               TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_undo ON events(seq) WHERE is_undo_stop = 1;
`

// ---- construction -----------------------------------------------------------

func initPermSchema(db *sql.DB) error {
	if _, err := db.Exec(`PRAGMA foreign_keys = ON`); err != nil {
		return fmt.Errorf("enable foreign keys: %w", err)
	}
	if _, err := db.Exec(permSchema); err != nil {
		return fmt.Errorf("init perm schema: %w", err)
	}
	return nil
}

func initMemSchema(db *sql.DB) error {
	if _, err := db.Exec(memSchema); err != nil {
		return fmt.Errorf("init mem schema: %w", err)
	}
	return nil
}

func openPerm(path string) (*sql.DB, error) {
	dsn := fmt.Sprintf("file:%s?_foreign_keys=on", path)
	db, err := sql.Open("sqlite3", dsn)
	if err != nil {
		return nil, err
	}
	if err := initPermSchema(db); err != nil {
		db.Close()
		return nil, err
	}
	return db, nil
}

func openMem() (*sql.DB, error) {
	db, err := sql.Open("sqlite3", ":memory:")
	if err != nil {
		return nil, fmt.Errorf("open mem journal: %w", err)
	}
	if err := initMemSchema(db); err != nil {
		db.Close()
		return nil, err
	}
	return db, nil
}

// Open opens (or creates) the on-disk rune.db and an in-memory journal.
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
		// Fall back to a second :memory: DB for perm.
		var err error
		perm, err = sql.Open("sqlite3", ":memory:")
		if err != nil {
			return nil, "", fmt.Errorf("open fallback perm db: %w", err)
		}
		if err := initPermSchema(perm); err != nil {
			perm.Close()
			return nil, "", fmt.Errorf("init fallback perm schema: %w", err)
		}
		warning = "history disabled — storage unavailable"
	}

	mem, err := openMem()
	if err != nil {
		perm.Close()
		return nil, "", err
	}

	return &Store{
		perm:       perm,
		mem:        mem,
		clock:      time.Now,
		currentSeq: math.MaxInt64,
	}, warning, nil
}

// OpenInMemory creates a Store with both perm and mem databases in :memory:.
// Useful for testing and fuzzing — no disk I/O, no path threading required.
func OpenInMemory(clock func() time.Time) (*Store, error) {
	perm, err := sql.Open("sqlite3", ":memory:")
	if err != nil {
		return nil, fmt.Errorf("open in-memory perm db: %w", err)
	}
	if err := initPermSchema(perm); err != nil {
		perm.Close()
		return nil, fmt.Errorf("init in-memory perm schema: %w", err)
	}

	mem, err := openMem()
	if err != nil {
		perm.Close()
		return nil, err
	}

	if clock == nil {
		clock = time.Now
	}
	return &Store{
		perm:       perm,
		mem:        mem,
		clock:      clock,
		currentSeq: math.MaxInt64,
	}, nil
}

// OpenAt creates a Store backed by baseDir/rune.db plus an in-memory journal.
// Intended for Phase-2 DATA-LOSS invariants that need real files.
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
		perm, err = sql.Open("sqlite3", ":memory:")
		if err != nil {
			return nil, "", fmt.Errorf("open fallback perm db: %w", err)
		}
		if err := initPermSchema(perm); err != nil {
			perm.Close()
			return nil, "", fmt.Errorf("init fallback perm schema: %w", err)
		}
		warning = "history disabled — storage unavailable"
	}

	mem, err := openMem()
	if err != nil {
		perm.Close()
		return nil, "", err
	}

	return &Store{
		perm:       perm,
		mem:        mem,
		clock:      time.Now,
		currentSeq: math.MaxInt64,
	}, warning, nil
}

// SetClock replaces the Store's clock function. Used to inject a logical clock
// for deterministic fuzzing (300 ms coalesce window is clock-driven).
func (s *Store) SetClock(clock func() time.Time) {
	s.clock = clock
}

// NewTestStore opens a Store suitable for tests: perm backed by a temp file,
// journal in :memory:. The store is closed automatically via t.Cleanup.
func NewTestStore(t *testing.T) *Store {
	t.Helper()
	permPath := filepath.Join(t.TempDir(), "rune_test.db")

	perm, err := openPerm(permPath)
	if err != nil {
		t.Fatalf("NewTestStore: open perm: %v", err)
	}

	mem, err := openMem()
	if err != nil {
		perm.Close()
		t.Fatalf("NewTestStore: open mem: %v", err)
	}

	s := &Store{
		perm:       perm,
		mem:        mem,
		clock:      time.Now,
		currentSeq: math.MaxInt64,
	}
	t.Cleanup(func() {
		if err := s.Close(); err != nil {
			t.Logf("NewTestStore cleanup: %v", err)
		}
	})
	return s
}

// Close closes both underlying database connections.
func (s *Store) Close() error {
	var permErr, memErr error
	if s.perm != nil {
		permErr = s.perm.Close()
	}
	if s.mem != nil {
		memErr = s.mem.Close()
	}
	if permErr != nil {
		return fmt.Errorf("close perm db: %w", permErr)
	}
	if memErr != nil {
		return fmt.Errorf("close mem db: %w", memErr)
	}
	return nil
}

// ---- documents --------------------------------------------------------------

// EnsureDocument returns the row id for path, inserting a new row if absent.
func (s *Store) EnsureDocument(path string) (int64, error) {
	at := s.clock().UTC().Format(time.RFC3339Nano)

	_, err := s.perm.Exec(
		`INSERT OR IGNORE INTO documents(path, created_at, last_seen_at) VALUES(?,?,?)`,
		path, at, at,
	)
	if err != nil {
		return 0, fmt.Errorf("ensure document %q: insert: %w", path, err)
	}

	_, err = s.perm.Exec(
		`UPDATE documents SET last_seen_at=? WHERE path=?`,
		at, path,
	)
	if err != nil {
		return 0, fmt.Errorf("ensure document %q: update last_seen_at: %w", path, err)
	}

	var id int64
	err = s.perm.QueryRow(`SELECT id FROM documents WHERE path=?`, path).Scan(&id)
	if err != nil {
		return 0, fmt.Errorf("ensure document %q: select id: %w", path, err)
	}
	return id, nil
}

