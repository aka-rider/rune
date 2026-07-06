package docstate

import (
	"database/sql"
	"errors"
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
	// fs is the ONE injected filesystem for every disk operation the store
	// performs — file-identity stats (OpenPath/Bind) AND, from WP4, the
	// Load/Probe/Materialize disk layer. A nil fs defaults to vfs.Disk (real
	// disk); the fuzz harness / workspace bootstrap inject a shared vfs.Mem
	// via UseFS so the store and workspace resolve identity and disk state
	// against the SAME files (closes S6 once workspace.WithFS and
	// docstate.UseFS share one value at bootstrap — WP5).
	fs vfs.FS
	// degraded is true ONLY for the in-memory fallback Open/OpenAt take when
	// the real on-disk database could not be opened (permissions, disk full,
	// etc.) — never for an intentional OpenInMemory (fuzz/tests). Degraded()
	// drives a persistent footer banner and a confirmation guard before every
	// Materialize (capture-into-RAM must never masquerade as durability).
	degraded bool
	// sessionID is this Store's own row in `sessions` (v10) — established
	// once at construction (Open/OpenAt/OpenInMemory/NewTestStore) and never
	// mutated after. It gives every journaled edit and recorded observation a
	// process identity, so two Store handles sharing one rune.db (two rune
	// windows on the same workDir) tell their own history apart from each
	// other instead of racing a single shared journal — see liveness.go and
	// CONSTITUTION.md §12. Every session-scoped method (AppendEdit,
	// RecoverDocument, HasHistory, UndoPeek/RedoPeek/MoveUndoPos, CurrentSeq,
	// SavedObs, Sync/ancestorAt) filters/joins on this internally; no
	// caller-visible signature changes anywhere outside this package.
	sessionID int64
	// livenessCheck decides whether a RECORDED (pid, proc_started_at) pair
	// still identifies a running process — real isProcessAlive in
	// production, overridable via SetLivenessCheck so tests can simulate a
	// dead session deterministically (mirrors the SetClock pattern) without
	// spawning and killing a real process. Consulted only by Load's
	// cross-session inheritance decision and RecoverAcrossSessions — never
	// by the dead-session reaper, which runs before a Store exists and takes
	// its own liveness function as a parameter.
	livenessCheck func(pid int, startedAt string) bool
}

// SetLivenessCheck overrides how this Store decides whether a different
// session's recorded process is still alive — Load's cross-session
// inheritance decision and RecoverAcrossSessions are the only consumers.
// Used in tests to simulate a dead (or alive) session deterministically,
// mirroring SetClock.
func (s *Store) SetLivenessCheck(fn func(pid int, startedAt string) bool) {
	s.livenessCheck = fn
}

// UseFS injects the filesystem used for every disk operation the store
// performs. Production leaves it as the default Disk; the session fuzzer
// sets a shared vfs.Mem so document identity and disk state match the
// in-memory files the workspace writes.
func (s *Store) UseFS(fs vfs.FS) { s.fs = fs }

// fsys returns the active filesystem, defaulting to real disk.
func (s *Store) fsys() vfs.FS {
	if s.fs == nil {
		return vfs.Disk{}
	}
	return s.fs
}

// statID returns the (inode, device) identity of path via the injected FS,
// defaulting to real disk. ok is false when stat fails or identity is
// unavailable, in which case the caller degrades to path-keying.
func (s *Store) statID(path string) (inode, device uint64, ok bool) {
	fi, err := s.fsys().Stat(path)
	if err != nil {
		return 0, 0, false
	}
	return vfs.FileID(fi)
}

// Degraded reports whether this Store is the in-memory fallback taken when
// the real on-disk database could not be opened. See the Store.degraded doc
// comment.
func (s *Store) Degraded() bool { return s.degraded }

// DocRef is returned by OpenPath and CreateScratch.
// ID is the stable VFS document primary key.
// RenamedFrom is set when OpenPath detects that the file was renamed since the
// VFS last saw it.
type DocRef struct {
	ID          int64
	RenamedFrom string // non-empty when a rename was detected
}

// ---- schema & low-level DSN construction ----
//
// schemaVersion, permSchema, sqliteURIPath, dropIfStale, readUserVersion,
// openPerm, openInMemPerm, and initPermSchema live in store_schema.go (§1.6,
// split out to stay under the 500-LoC limit — this file now holds the Store
// type, filesystem/identity accessors, and the Open/Close lifecycle that
// build on top of that schema layer).

// ensureRuneDir creates runeDir (workDir/.rune) if it doesn't already exist
// and, ONLY when freshly created, seeds a .gitignore containing "*" inside
// it — self-contained (works whether or not workDir is a git repo, never
// touches the user's own .gitignore) so a vault that happens to be a git
// repo never shows .rune/ as untracked. An already-existing runeDir is left
// untouched (no re-seeding, no clobbering a user's own edits to that file).
func ensureRuneDir(runeDir string) error {
	if _, err := os.Stat(runeDir); err == nil {
		return nil
	} else if !errors.Is(err, os.ErrNotExist) {
		return fmt.Errorf("stat %q: %w", runeDir, err)
	}
	if err := os.MkdirAll(runeDir, 0o700); err != nil {
		return fmt.Errorf("mkdir %q: %w", runeDir, err)
	}
	if err := os.WriteFile(filepath.Join(runeDir, ".gitignore"), []byte("*\n"), 0o644); err != nil {
		return fmt.Errorf("write %q: %w", filepath.Join(runeDir, ".gitignore"), err)
	}
	return nil
}

// Open opens (or creates) workDir/.rune/rune.db and starts the VFS store.
// Different workDirs never share a database — unrelated vaults never
// contend. Opening the SAME workDir from multiple rune processes is also
// normal usage, not an edge case (§12 Standing Decisions: "Per-workspace
// store, SQLite-native concurrency") — but only because every mutating
// method is session-scoped (v10): each Open call establishes its own
// `sessions` row and journals/observes under its own session_id, so two
// windows on the same file never race a single shared journal the way an
// unscoped design would. It is NOT "normal usage by omission" — see
// liveness.go and CONSTITUTION.md §12 for the actual mechanism.
//
// The returned string is a non-fatal degradation warning; callers may surface
// it to the user but MUST NOT fail on a non-empty warning. The error return is
// reserved for the near-degenerate case where even the :memory: fallback
// itself cannot be opened, OR this process's own session row cannot be
// established (session identity is now load-bearing for every write this
// Store will ever perform) — there is no fallback left below that.
//
// Concurrent opens of the SAME workDir are arbitrated by SQLite's own locking
// (_txlock=immediate + _busy_timeout=5000 on openPerm's DSN), not a custom
// flock — see the invariant every mutating Store method must uphold,
// documented in CONSTITUTION.md §12.
//
// Open ladder:
//  1. Try workDir/.rune/rune.db
//  2. On failure: ensureRuneDir (mkdir + seed .gitignore on first creation) and retry
//  3. On failure: fall back to :memory: for perm + set warning
//  4. Establish this process's own session row (+ best-effort dead-session
//     reaping); hard fail only if even THAT fails on top of the :memory:
//     fallback already having been taken.
func Open(workDir string) (*Store, string, error) {
	runeDir := filepath.Join(workDir, ".rune")
	permPath := filepath.Join(runeDir, "rune.db")
	return openStoreAt(permPath, func() error { return ensureRuneDir(runeDir) })
}

// OpenAt creates a Store backed by baseDir/rune.db.
// Intended for tests that need real files (durability, crash-recovery).
func OpenAt(baseDir string) (*Store, string, error) {
	permPath := filepath.Join(baseDir, "rune.db")
	return openStoreAt(permPath, func() error { return os.MkdirAll(baseDir, 0o700) })
}

// openStoreAt is the shared open ladder for Open/OpenAt: open, retrying once
// via mkdirRetry on failure, then degrading to an in-memory perm db (with a
// warning) if the real file still can't be opened. Every failure mode up to
// that point degrades uniformly; establishing this process's OWN session row
// is the one remaining hard-fail case (v10 — session identity is load-
// bearing for every subsequent write), alongside the fallback :memory: open
// itself failing (see Open's doc comment). The dead-session reaper runs
// once here too, best-effort (never blocks Open on a reaper error — mirrors
// GCEmptyScratch's own fire-and-forget housekeeping precedent).
func openStoreAt(permPath string, mkdirRetry func() error) (*Store, string, error) {
	perm, permErr := openPerm(permPath)
	if permErr != nil {
		if mkErr := mkdirRetry(); mkErr == nil {
			perm, permErr = openPerm(permPath)
		}
	}

	var warning string
	var degraded bool
	if permErr != nil {
		var err error
		perm, err = openInMemPerm()
		if err != nil {
			return nil, "", fmt.Errorf("open fallback perm db: %w", err)
		}
		warning = "history disabled — storage unavailable"
		degraded = true
	}

	sessionID, err := establishSession(perm, time.Now)
	if err != nil {
		return nil, "", fmt.Errorf("establish session: %w", err)
	}
	store := &Store{perm: perm, clock: time.Now, degraded: degraded, sessionID: sessionID, livenessCheck: isProcessAlive}
	if rerr := reapDeadSessions(perm, store.livenessCheck); rerr != nil {
		_ = rerr // fire-and-forget: best-effort housekeeping (R1), never blocks Open
	}

	return store, warning, nil
}

// OpenInMemory creates a Store with the perm database in :memory:.
// Useful for testing and fuzzing — no disk I/O, no path threading required,
// and no lock (there is no real file to protect).
func OpenInMemory(clock func() time.Time) (*Store, error) {
	perm, err := openInMemPerm()
	if err != nil {
		return nil, err
	}
	if clock == nil {
		clock = time.Now
	}
	sessionID, err := establishSession(perm, clock)
	if err != nil {
		return nil, fmt.Errorf("open in-memory store: establish session: %w", err)
	}
	return &Store{perm: perm, clock: clock, sessionID: sessionID, livenessCheck: isProcessAlive}, nil
}

// SetClock replaces the Store's clock function. Used in deterministic tests.
func (s *Store) SetClock(clock func() time.Time) {
	s.clock = clock
}

// NewTestStore opens a Store suitable for tests: perm backed by a temp file,
// opened exactly like a real Open/OpenAt (minus the workDir/baseDir
// indirection — tests get exact path control). The store is closed
// automatically via t.Cleanup.
func NewTestStore(t *testing.T) *Store {
	t.Helper()
	permPath := filepath.Join(t.TempDir(), "rune_test.db")

	perm, err := openPerm(permPath)
	if err != nil {
		t.Fatalf("NewTestStore: open perm: %v", err)
	}
	sessionID, err := establishSession(perm, time.Now)
	if err != nil {
		t.Fatalf("NewTestStore: establish session: %v", err)
	}

	s := &Store{perm: perm, clock: time.Now, sessionID: sessionID, livenessCheck: isProcessAlive}
	t.Cleanup(func() {
		if err := s.Close(); err != nil {
			t.Logf("NewTestStore cleanup: %v", err)
		}
	})
	return s
}

// Close closes the underlying database connection.
func (s *Store) Close() error {
	if s.perm == nil {
		return nil
	}
	if err := s.perm.Close(); err != nil {
		return fmt.Errorf("close perm db: %w", err)
	}
	return nil
}
