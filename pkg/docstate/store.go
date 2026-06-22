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
