package docstate

import (
	"database/sql"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	_ "github.com/mattn/go-sqlite3"

	"rune/pkg/editor/buffer"
)

// oldV3Schema is the pre-v4 schema shape (surface/kind/is_undo_stop/
// anchor_snapshot_id/focus_* on events; no documents.kind), reproduced here
// ONLY to build a realistic legacy fixture for the drop-migration test below.
// It must NOT be kept in sync with permSchema — its entire purpose is to be
// stale.
const oldV3Schema = `
CREATE TABLE documents (
	id           INTEGER PRIMARY KEY,
	path         TEXT    NOT NULL DEFAULT '',
	inode        INTEGER,
	device       INTEGER,
	current_seq  INTEGER,
	saved_seq    INTEGER,
	created_at   TEXT    NOT NULL,
	last_seen_at TEXT    NOT NULL
);
CREATE TABLE blobs (
	hash    TEXT PRIMARY KEY,
	content BLOB NOT NULL
);
CREATE TABLE snapshots (
	id         INTEGER PRIMARY KEY,
	doc_id     INTEGER NOT NULL,
	blob_hash  TEXT    NOT NULL,
	source     TEXT    NOT NULL,
	seq        INTEGER NOT NULL DEFAULT 0,
	created_at TEXT    NOT NULL
);
CREATE TABLE events (
	seq                INTEGER PRIMARY KEY AUTOINCREMENT,
	doc_id             INTEGER NOT NULL,
	surface            TEXT    NOT NULL,
	kind               TEXT    NOT NULL,
	edits              BLOB,
	cursors_before     BLOB,
	cursors_after      BLOB,
	focus_before       TEXT,
	focus_after        TEXT,
	is_undo_stop       INTEGER NOT NULL DEFAULT 0,
	anchor_snapshot_id INTEGER,
	at                 TEXT    NOT NULL
);
CREATE TABLE drafts (
	surface    TEXT PRIMARY KEY,
	content    TEXT NOT NULL,
	updated_at TEXT NOT NULL
);
CREATE TABLE search_history (
	query        TEXT PRIMARY KEY,
	last_used_at TEXT NOT NULL
);
`

// TestDropMigration_FreshStoreIsCurrentVersion: a brand-new store (no prior
// file) lands directly at the current schema version.
func TestDropMigration_FreshStoreIsCurrentVersion(t *testing.T) {
	s := NewTestStore(t)
	var version int
	if err := s.perm.QueryRow(`PRAGMA user_version`).Scan(&version); err != nil {
		t.Fatalf("read user_version: %v", err)
	}
	if version != schemaVersion {
		t.Errorf("user_version = %d, want %d", version, schemaVersion)
	}
}

// TestDropMigration_LegacyV3FixtureIsDropped hand-builds a v3-shaped database
// file (raw SQL, including a poisoned row: a title-surface event journaled
// against a FILE document — exactly the S1 corruption vector this plan
// closes) and confirms opening it through the real Open path deletes and
// recreates the file rather than patching the legacy shape in place: zero
// legacy rows survive, and the new schema (no `surface`/`kind`/`is_undo_stop`
// columns on events, `kind` present on documents) is what's actually there.
func TestDropMigration_LegacyV3FixtureIsDropped(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "rune.db")

	// Build the fixture directly against the file, bypassing every docstate
	// constructor (they all apply the CURRENT schema).
	raw, err := sql.Open("sqlite3", "file:"+path)
	if err != nil {
		t.Fatalf("open fixture: %v", err)
	}
	if _, err := raw.Exec(oldV3Schema); err != nil {
		t.Fatalf("create v3 schema: %v", err)
	}
	if _, err := raw.Exec(`PRAGMA user_version = 3`); err != nil {
		t.Fatalf("set legacy user_version: %v", err)
	}
	res, err := raw.Exec(
		`INSERT INTO documents(path, inode, device, current_seq, saved_seq, created_at, last_seen_at)
		 VALUES('/legacy/file.md', 42, 1, NULL, NULL, '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z')`,
	)
	if err != nil {
		t.Fatalf("seed legacy document: %v", err)
	}
	legacyDocID, _ := res.LastInsertId()
	// The poisoned row: a title-surface event journaled against the FILE
	// doc's id — the exact S1 shape (title/chat splicing into file content
	// at foreign byte offsets) this plan's v4 model closes at the root.
	if _, err := raw.Exec(
		`INSERT INTO events(doc_id, surface, kind, edits, is_undo_stop, at)
		 VALUES(?, 'title', 'edit', '[]', 1, '2020-01-01T00:00:01Z')`,
		legacyDocID,
	); err != nil {
		t.Fatalf("seed poisoned title event: %v", err)
	}
	if err := raw.Close(); err != nil {
		t.Fatalf("close fixture: %v", err)
	}

	// Open through the real path.
	store, warn, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("OpenAt: %v", err)
	}
	defer store.Close()
	if warn != "" {
		t.Errorf("unexpected degradation warning: %q", warn)
	}

	var version int
	if err := store.perm.QueryRow(`PRAGMA user_version`).Scan(&version); err != nil {
		t.Fatalf("read user_version: %v", err)
	}
	if version != schemaVersion {
		t.Errorf("user_version after drop-migration = %d, want %d", version, schemaVersion)
	}

	// Zero legacy rows remain — the file was deleted and recreated, not
	// patched in place.
	if n := countRows(t, store.perm, `SELECT COUNT(*) FROM documents`); n != 0 {
		t.Errorf("documents: got %d rows after drop-migration, want 0", n)
	}
	if n := countRows(t, store.perm, `SELECT COUNT(*) FROM events`); n != 0 {
		t.Errorf("events: got %d rows after drop-migration, want 0", n)
	}

	// The new shape is actually in effect: events has no surface/kind/
	// is_undo_stop columns; documents has kind.
	for _, col := range []string{"surface", "kind", "is_undo_stop", "anchor_snapshot_id", "focus_before", "focus_after"} {
		if hasColumn(t, store.perm, "events", col) {
			t.Errorf("events.%s should not exist after drop-migration to v4", col)
		}
	}
	if !hasColumn(t, store.perm, "documents", "kind") {
		t.Error("documents.kind should exist after drop-migration to v4")
	}

	// The store is fully functional post-migration.
	ref, err := store.OpenPath("/fresh.md")
	if err != nil {
		t.Fatalf("store not functional after drop-migration: OpenPath: %v", err)
	}
	if _, err := store.AppendEdit(ref.ID, singleInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("store not functional after drop-migration: AppendEdit: %v", err)
	}
}

// hasColumn reports whether table has a column named col.
func hasColumn(t *testing.T, db *sql.DB, table, col string) bool {
	t.Helper()
	rows, err := db.Query(`PRAGMA table_info(` + table + `)`)
	if err != nil {
		t.Fatalf("PRAGMA table_info(%s): %v", table, err)
	}
	defer rows.Close()
	for rows.Next() {
		var cid int
		var name, ctype string
		var notnull, pk int
		var dflt sql.NullString
		if err := rows.Scan(&cid, &name, &ctype, &notnull, &dflt, &pk); err != nil {
			t.Fatalf("scan table_info: %v", err)
		}
		if name == col {
			return true
		}
	}
	return false
}

// ---- SQLite-native concurrency (per-workspace store, no custom flock) ------
//
// docstate.Open is per-workspace (workDir/.rune/rune.db): different workDirs
// never share a database, so unrelated vaults never contend at all.
// Concurrent opens of the SAME workDir are no longer refused by a sidecar
// flock — they are arbitrated by SQLite's own locking (_txlock=immediate +
// _busy_timeout=5000 on openPerm's DSN), which is safe if and only if every
// mutating Store method reads whatever state it uses to decide what to write
// INSIDE the same transaction it writes in (§12 Standing Decisions). The
// tests below prove that invariant holds for both the common write path
// (AppendEdit) and the two methods that were missing it until this change
// (openPathByInode / openPathByName — design point 4).

// TestOpenAtSameDir_ConcurrentAppendEditBothPersist proves the plan's core
// safety claim — but, since two-editors-session-scoped-journal (v10), for a
// DIFFERENT reason than when this test was first written. "Both persist"
// (2*n events physically exist afterward) is no longer a claim about "SQL
// is transactional" alone — with two independent *Store handles now also
// being two independent SESSIONS (session_id), the real claim is that each
// session's own events are properly ATTRIBUTED and ISOLATED: first's own 25
// AppendEdit calls and second's own 25 land as 50 total rows (this part
// mechanically unchanged from before), but each session's OWN
// RecoverDocument reflects ONLY the edits IT journaled, never the other's —
// the direct fix for the root-cause bug this plan closes (undo/redo and
// coalescing could previously reach across a shared journal into a
// DIFFERENT process's keystrokes). Two independent *Store handles (two
// separate OpenAt calls against the SAME directory — NOT two goroutines
// sharing one handle; SetMaxOpenConns(1) means only genuinely distinct
// *sql.DB handles exercise real cross-connection SQLite locking) both open
// successfully (no flock, no refusal) and, when they concurrently
// AppendEdit against the SAME document, both writers' edits persist with no
// lost update AND with no cross-session bleed.
func TestOpenAtSameDir_ConcurrentAppendEditBothPersist(t *testing.T) {
	dir := t.TempDir()

	first, warn, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("first OpenAt: %v", err)
	}
	defer first.Close()
	if warn != "" {
		t.Fatalf("first OpenAt: unexpected warning %q", warn)
	}

	second, warn, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("second OpenAt same dir: %v — must succeed now, no ErrLocked", err)
	}
	defer second.Close()
	if warn != "" {
		t.Fatalf("second OpenAt: unexpected warning %q", warn)
	}

	// docID is created once via `first` and then shared by BOTH stores below
	// — mirroring how two rune windows on the same real file resolve to the
	// same docID via OpenPath's inode-keying (store_documents.go), each from
	// its own independent Store/session.
	docID := testDoc(t, first)

	const n = 25
	var wg sync.WaitGroup
	errCh := make(chan error, 2*n)
	wg.Add(2)
	go func() {
		defer wg.Done()
		for i := range n {
			edit := []buffer.AppliedEdit{{Insert: fmt.Sprintf("first-%02d", i)}}
			if _, err := first.AppendEdit(docID, edit, nil, nil); err != nil {
				errCh <- fmt.Errorf("first.AppendEdit %d: %w", i, err)
			}
		}
	}()
	go func() {
		defer wg.Done()
		for i := range n {
			edit := []buffer.AppliedEdit{{Insert: fmt.Sprintf("second-%02d", i)}}
			if _, err := second.AppendEdit(docID, edit, nil, nil); err != nil {
				errCh <- fmt.Errorf("second.AppendEdit %d: %w", i, err)
			}
		}
	}()
	wg.Wait()
	close(errCh)
	for err := range errCh {
		t.Errorf("concurrent AppendEdit from two independent stores: %v", err)
	}

	edits, err := first.AllEdits(docID)
	if err != nil {
		t.Fatalf("AllEdits: %v", err)
	}
	if len(edits) != 2*n {
		t.Fatalf("expected %d persisted events from two concurrent writers, got %d — lost update or corruption", 2*n, len(edits))
	}

	// Isolation (v10, the actual fix this plan makes true): each session's
	// OWN RecoverDocument reflects ONLY the edits IT journaled — never
	// corrupted, coalesced with, or overwritten by the other session's
	// concurrent keystrokes to the same docID.
	firstRecovered, err := first.RecoverDocument(docID)
	if err != nil {
		t.Fatalf("first.RecoverDocument: %v", err)
	}
	secondRecovered, err := second.RecoverDocument(docID)
	if err != nil {
		t.Fatalf("second.RecoverDocument: %v", err)
	}
	if strings.Contains(firstRecovered, "second-") {
		t.Fatalf("first.RecoverDocument contains second's content — cross-session bleed: %q", firstRecovered)
	}
	if strings.Contains(secondRecovered, "first-") {
		t.Fatalf("second.RecoverDocument contains first's content — cross-session bleed: %q", secondRecovered)
	}
	for i := range n {
		marker := fmt.Sprintf("first-%02d", i)
		if !strings.Contains(firstRecovered, marker) {
			t.Fatalf("first.RecoverDocument missing its own edit %q — lost update within a session: %q", marker, firstRecovered)
		}
		marker = fmt.Sprintf("second-%02d", i)
		if !strings.Contains(secondRecovered, marker) {
			t.Fatalf("second.RecoverDocument missing its own edit %q — lost update within a session: %q", marker, secondRecovered)
		}
	}
}

// TestOpenPath_ConcurrentSameNewFile_InodeKeyed_ResolvesToSameDocID is the
// test that actually proves design point 4's transactional fix — not the
// AppendEdit test above, which only ever exercises a document that already
// exists. Two independent OpenAt-opened stores both call OpenPath on the
// SAME never-before-seen file at (as close to) the same time.
//
// Before the fix, openPathByInode ran a bare, non-transactional SELECT to
// decide new-row-vs-existing-row and only opened a transaction afterward for
// the write — two callers could both miss the SELECT and both attempt the
// INSERT, with the loser hitting idx_documents_inode's UNIQUE constraint
// instead of getting a docID. After the fix (Begin() wraps the whole
// decide-then-write sequence, before the SELECT), the second caller's
// Begin() blocks at _txlock=immediate until the first commits, then its own
// SELECT finds the now-existing row and takes the found-row branch instead
// of racing the INSERT.
func TestOpenPath_ConcurrentSameNewFile_InodeKeyed_ResolvesToSameDocID(t *testing.T) {
	dir := t.TempDir()
	first, _, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("first OpenAt: %v", err)
	}
	defer first.Close()
	second, _, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("second OpenAt: %v", err)
	}
	defer second.Close()

	path := filepath.Join(dir, "new-file.md")
	if err := os.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatal(err)
	}

	var refs [2]DocRef
	var errs [2]error
	var wg sync.WaitGroup
	wg.Add(2)
	go func() { defer wg.Done(); refs[0], errs[0] = first.OpenPath(path) }()
	go func() { defer wg.Done(); refs[1], errs[1] = second.OpenPath(path) }()
	wg.Wait()

	if errs[0] != nil {
		t.Fatalf("first.OpenPath: %v", errs[0])
	}
	if errs[1] != nil {
		t.Fatalf("second.OpenPath: %v", errs[1])
	}
	if refs[0].ID != refs[1].ID {
		t.Fatalf("concurrent OpenPath on the same new file resolved to different doc IDs: %d vs %d", refs[0].ID, refs[1].ID)
	}
	if n := countRows(t, first.perm, `SELECT COUNT(*) FROM documents`); n != 1 {
		t.Fatalf("expected exactly 1 documents row for the raced file, got %d — UNIQUE constraint race produced a duplicate", n)
	}
}

// TestOpenPathByName_ConcurrentSameNewPath_ResolvesToSameDocID exercises the
// path-keyed fallback half of design point 4 (openPathByName, used when
// stat can't obtain a real inode) the same way the test above exercises
// openPathByInode: two independent stores racing OpenPath's internal
// path-keyed fallback on the SAME never-before-seen path must resolve to the
// same doc ID, not a UNIQUE-constraint error on idx_documents_path.
func TestOpenPathByName_ConcurrentSameNewPath_ResolvesToSameDocID(t *testing.T) {
	dir := t.TempDir()
	first, _, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("first OpenAt: %v", err)
	}
	defer first.Close()
	second, _, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("second OpenAt: %v", err)
	}
	defer second.Close()

	// A path with nothing on disk to stat — this is what routes OpenPath's
	// internal dispatch into openPathByName instead of openPathByInode; called
	// directly here (this is a white-box package test) to pin the fallback
	// itself rather than depend on statID's failure mode on this platform.
	const path = "/nonexistent/never-seen.md"
	const at = "2026-01-01T00:00:00Z"

	var refs [2]DocRef
	var errs [2]error
	var wg sync.WaitGroup
	wg.Add(2)
	go func() { defer wg.Done(); refs[0], errs[0] = first.openPathByName(path, at) }()
	go func() { defer wg.Done(); refs[1], errs[1] = second.openPathByName(path, at) }()
	wg.Wait()

	if errs[0] != nil {
		t.Fatalf("first.openPathByName: %v", errs[0])
	}
	if errs[1] != nil {
		t.Fatalf("second.openPathByName: %v", errs[1])
	}
	if refs[0].ID != refs[1].ID {
		t.Fatalf("concurrent openPathByName on the same new path resolved to different doc IDs: %d vs %d", refs[0].ID, refs[1].ID)
	}
	if n := countRows(t, first.perm, `SELECT COUNT(*) FROM documents WHERE path=?`, path); n != 1 {
		t.Fatalf("expected exactly 1 documents row for the raced path, got %d — UNIQUE constraint race produced a duplicate", n)
	}
}

// TestLock_InMemoryStoresNeverConflict: OpenInMemory never touches a real
// file — any number of in-memory stores coexist freely.
func TestLock_InMemoryStoresNeverConflict(t *testing.T) {
	s1, err := OpenInMemory(nil)
	if err != nil {
		t.Fatalf("OpenInMemory 1: %v", err)
	}
	defer s1.Close()
	s2, err := OpenInMemory(nil)
	if err != nil {
		t.Fatalf("OpenInMemory 2: %v", err)
	}
	defer s2.Close()
}

// TestOpen_DegradesRepeatedlyOnPermissionFailure: when the real path can't
// be opened (no permissions), Open degrades to an in-memory perm db with a
// warning — and a second Open against the SAME unusable workDir degrades
// again rather than hard-failing (point 2: every failure mode now degrades
// uniformly; there is no lock left to refuse on).
func TestOpen_DegradesRepeatedlyOnPermissionFailure(t *testing.T) {
	noPermDir := t.TempDir()
	if err := os.Chmod(noPermDir, 0o000); err != nil {
		t.Skip("cannot chmod temp dir:", err)
	}
	t.Cleanup(func() { os.Chmod(noPermDir, 0o700) })

	s1, warn1, err := Open(noPermDir)
	if err != nil {
		t.Fatalf("first Open: %v", err)
	}
	defer s1.Close()
	if warn1 == "" {
		t.Fatal("expected degradation warning on first Open")
	}

	s2, warn2, err := Open(noPermDir)
	if err != nil {
		t.Fatalf("second Open: %v (should degrade again, not hard-fail)", err)
	}
	defer s2.Close()
	if warn2 == "" {
		t.Fatal("expected degradation warning on second Open")
	}
}
