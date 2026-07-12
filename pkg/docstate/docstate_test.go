package docstate

import (
	"database/sql"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"rune/internal/editortest"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

// ---- helpers ----------------------------------------------------------------
// Shared by every _test.go file in this package.

// newStoreAtPath creates a Store backed by a real file at permPath, with its
// own established session (v10 — every write is now session-scoped, so a
// bare &Store{} with no session row would fail every FK-checked insert).
// The caller is responsible for closing the store.
func newStoreAtPath(t *testing.T, permPath string) *Store {
	t.Helper()
	perm, err := openPerm(permPath)
	if err != nil {
		t.Fatalf("newStoreAtPath: openPerm %q: %v", permPath, err)
	}
	sessionID, err := establishSession(perm, time.Now)
	if err != nil {
		t.Fatalf("newStoreAtPath: establish session: %v", err)
	}
	return &Store{perm: perm, clock: time.Now, sessionID: sessionID, livenessCheck: isProcessAlive}
}

// countRows returns the row count for the given query.
func countRows(t *testing.T, db *sql.DB, query string, args ...any) int {
	t.Helper()
	var n int
	if err := db.QueryRow(query, args...).Scan(&n); err != nil {
		t.Fatalf("countRows %q: %v", query, err)
	}
	return n
}

// latestSnapshotContent reads the RAW content of the most recent snapshot row
// for docID, bypassing RecoverDocument's edit-replay entirely — used to
// verify a snapshot itself survived durably (e.g. across a Close/reopen),
// independent of whatever the journal replay would separately reconstruct.
func latestSnapshotContent(t *testing.T, s *Store, docID int64) string {
	t.Helper()
	var hash string
	if err := s.perm.QueryRow(
		`SELECT blob_hash FROM snapshots WHERE doc_id=? ORDER BY id DESC LIMIT 1`,
		docID,
	).Scan(&hash); err != nil {
		t.Fatalf("latestSnapshotContent: query snapshot hash: %v", err)
	}
	content, err := s.GetBlob(hash)
	if err != nil {
		t.Fatalf("latestSnapshotContent: GetBlob: %v", err)
	}
	return content
}

// fixedClock replaces s's wall clock with a deterministic one (through
// SetClock, the exported seam — never a direct s.clock poke) and returns an
// advance function. The single home for the fixed-clock pattern the
// coalescing/snapshot/edit-range tests all repeat.
func fixedClock(s *Store) func(time.Duration) {
	c := editortest.NewClock()
	s.SetClock(func() time.Time { return c.Now() })
	return func(d time.Duration) { c = c.Advance(d) }
}

// seedCorruptEvent inserts a raw event row whose edits payload is
// deliberately unparseable — the single schema-coupled home for the
// corruption INSERT the §1.3 surfacing tests need (a corrupt row cannot be
// produced through the public API by definition).
func seedCorruptEvent(t *testing.T, s *Store, docID int64, payload string) {
	t.Helper()
	if _, err := s.perm.Exec(
		`INSERT INTO events(doc_id, session_id, edits, cursors_before, cursors_after, at) VALUES(?,?,?,?,?,?)`,
		docID, s.sessionID, payload, `[]`, `[]`, "2026-01-01T00:00:00Z",
	); err != nil {
		t.Fatalf("seed corrupt event: %v", err)
	}
}

// corruptBlob flips one byte of the stored (compressed) blob content behind
// hash — the single schema-coupled home for the blob-rot UPDATE (GetBlob
// re-verifies SHA-256 on every read; bit rot cannot be simulated through the
// public API).
func corruptBlob(t *testing.T, s *Store, hash string) {
	t.Helper()
	var compressed []byte
	if err := s.perm.QueryRow(`SELECT content FROM blobs WHERE hash=?`, hash).Scan(&compressed); err != nil {
		t.Fatalf("corruptBlob: read compressed blob: %v", err)
	}
	if len(compressed) == 0 {
		t.Fatal("corruptBlob: precondition: expected non-empty compressed blob")
	}
	corrupted := append([]byte(nil), compressed...)
	corrupted[len(corrupted)-1] ^= 0xFF
	if _, err := s.perm.Exec(`UPDATE blobs SET content=? WHERE hash=?`, corrupted, hash); err != nil {
		t.Fatalf("corruptBlob: %v", err)
	}
}

// singleInsert returns a one-element AppliedEdit slice that looks like a
// single character insertion at offset 0 with no deleted text.
func singleInsert(ch string) []buffer.AppliedEdit {
	return []buffer.AppliedEdit{{Start: 0, End: 0, Deleted: "", Insert: ch}}
}

// insertAt is singleInsert at an explicit byte offset — what a real typing
// run journals (each keystroke starts where the previous insert ended, which
// is also what the coalescing gate now requires; see canCoalesceInto).
func insertAt(pos int, ch string) []buffer.AppliedEdit {
	return []buffer.AppliedEdit{{Start: pos, End: pos, Deleted: "", Insert: ch}}
}

// undoStep / redoStep exercise the production undo/redo primitives: peek the
// target, then commit the journal move (MoveUndoPos) — the same peek→commit the
// workspace uses to keep the position coherent with the buffer (§1.4.8). They
// replicate the old advance-on-success UndoTarget/RedoTarget so existing
// positional assertions hold against the real primitives. A peek error (§1.3 —
// corrupt event, never silently folded into ok=false) fails the test loudly.
func undoStep(t *testing.T, s *Store, docID int64) (Step, bool) {
	t.Helper()
	step, ok, err := s.UndoPeek(docID)
	if err != nil {
		t.Fatalf("undoStep: UndoPeek: %v", err)
	}
	if ok {
		if err := s.MoveUndoPos(docID, step.NewPos); err != nil {
			t.Fatalf("undoStep: MoveUndoPos: %v", err)
		}
	}
	return step, ok
}

func redoStep(t *testing.T, s *Store, docID int64) (Step, bool) {
	t.Helper()
	step, ok, err := s.RedoPeek(docID)
	if err != nil {
		t.Fatalf("redoStep: RedoPeek: %v", err)
	}
	if ok {
		if err := s.MoveUndoPos(docID, step.NewPos); err != nil {
			t.Fatalf("redoStep: MoveUndoPos: %v", err)
		}
	}
	return step, ok
}

// testDoc creates a scratch document and returns its docID. Fatal on error.
func testDoc(t *testing.T, s *Store) int64 {
	t.Helper()
	ref, err := s.CreateScratch("")
	if err != nil {
		t.Fatalf("testDoc: CreateScratch: %v", err)
	}
	return ref.ID
}

var noCursors []cursor.Cursor

// ---- P0: integrity & persistence --------------------------------------------

func TestBlobRoundTrip(t *testing.T) {
	s := NewTestStore(t)

	cases := []struct {
		name    string
		content string
	}{
		{"ascii", "Hello, World! This is a test."},
		{"cjk", "日本語テスト"},
		{"emoji", "🎉🚀✨🌍"},
		{"large", strings.Repeat("abcdefghijklmnopqrstuvwxyz\n", 40000)}, // >1 MB
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			hash, err := s.PutBlob(tc.content)
			if err != nil {
				t.Fatalf("PutBlob: %v", err)
			}
			got, err := s.GetBlob(hash)
			if err != nil {
				t.Fatalf("GetBlob: %v", err)
			}
			if got != tc.content {
				t.Errorf("round-trip mismatch: got %d bytes, want %d bytes", len(got), len(tc.content))
			}
		})
	}
}

func TestBlobDeduplication(t *testing.T) {
	s := NewTestStore(t)
	content := "identical content"

	h1, err := s.PutBlob(content)
	if err != nil {
		t.Fatalf("first PutBlob: %v", err)
	}
	h2, err := s.PutBlob(content)
	if err != nil {
		t.Fatalf("second PutBlob: %v", err)
	}
	if h1 != h2 {
		t.Errorf("expected same hash for identical content, got %q vs %q", h1, h2)
	}

	n := countRows(t, s.perm, `SELECT COUNT(*) FROM blobs WHERE hash=?`, h1)
	if n != 1 {
		t.Errorf("expected exactly 1 blobs row, got %d", n)
	}
}

func TestFlushWritesSnapshot(t *testing.T) {
	s := NewTestStore(t)
	content := "the quick brown fox"

	ref, err := s.OpenPath("/test.md")
	if err != nil {
		t.Fatalf("OpenPath: %v", err)
	}
	docID := ref.ID

	snapID, err := s.CreateSnapshot(docID, content, 0)
	if err != nil {
		t.Fatalf("CreateSnapshot: %v", err)
	}
	if snapID <= 0 {
		t.Errorf("expected positive snapshot id, got %d", snapID)
	}

	// Assert exactly one snapshots row with the correct doc_id.
	var gotDocID int64
	if err := s.perm.QueryRow(
		`SELECT doc_id FROM snapshots WHERE id=?`, snapID,
	).Scan(&gotDocID); err != nil {
		t.Fatalf("query snapshots: %v", err)
	}
	if gotDocID != docID {
		t.Errorf("doc_id: got %d, want %d", gotDocID, docID)
	}

	// The snapshot row's raw blob reconstructs the original content.
	if got := latestSnapshotContent(t, s, docID); got != content {
		t.Errorf("latest snapshot content: got %q, want %q", got, content)
	}
}

func TestCrashRecovery(t *testing.T) {
	permPath := filepath.Join(t.TempDir(), "crash_test.db")

	const content = "document content after crash"
	const draftText = "in-progress chat prompt"
	var savedDocID int64

	// Session 1: write data and close.
	{
		s := newStoreAtPath(t, permPath)
		ref, err := s.OpenPath("/notes.md")
		if err != nil {
			t.Fatalf("OpenPath: %v", err)
		}
		savedDocID = ref.ID

		edits := singleInsert("a")
		if _, err := s.AppendEdit(savedDocID, edits, noCursors, noCursors); err != nil {
			t.Fatalf("AppendEdit: %v", err)
		}
		if _, err := s.CreateSnapshot(savedDocID, content, 0); err != nil {
			t.Fatalf("CreateSnapshot: %v", err)
		}
		if err := s.UpsertDraft("chat", draftText); err != nil {
			t.Fatalf("UpsertDraft: %v", err)
		}
		if err := s.Close(); err != nil {
			t.Fatalf("Close: %v", err)
		}
	}

	// Session 2: reopen and verify persisted state.
	{
		s := newStoreAtPath(t, permPath)
		defer s.Close()

		if got := latestSnapshotContent(t, s, savedDocID); got != content {
			t.Errorf("crash recovery: got %q, want %q", got, content)
		}

		draft, err := s.GetDraft("chat")
		if err != nil {
			t.Fatalf("GetDraft after reopen: %v", err)
		}
		if draft != draftText {
			t.Errorf("draft recovery: got %q, want %q", draft, draftText)
		}
	}
}

func TestOpenLadderDegradation(t *testing.T) {
	// Create a dir with no permissions so MkdirAll fails.
	noPermDir := t.TempDir()
	if err := os.Chmod(noPermDir, 0o000); err != nil {
		t.Skip("cannot chmod temp dir:", err)
	}
	t.Cleanup(func() { os.Chmod(noPermDir, 0o700) }) // restore for cleanup

	store, warn, err := Open(noPermDir)
	if err != nil {
		t.Fatalf("Open() returned error: %v", err)
	}
	defer store.Close()

	if !strings.Contains(warn, "history disabled") {
		t.Errorf("expected degradation warning containing %q, got %q", "history disabled", warn)
	}
	if store == nil {
		t.Fatal("Open() returned nil store despite warning")
	}

	// Store must be functional despite degradation.
	ref, err := store.OpenPath("/degraded.md")
	if err != nil {
		t.Fatalf("store not functional after degradation: OpenPath: %v", err)
	}
	if _, err := store.CreateSnapshot(ref.ID, "content", 0); err != nil {
		t.Fatalf("store not functional after degradation: CreateSnapshot: %v", err)
	}
}

// TestSchemaConstraints verifies the integrity constraints the v4 schema
// adds actually reject bad rows — FK enforcement, the documents.kind CHECK,
// and the seq >= 0 CHECK. Each raw INSERT/UPDATE MUST error.
func TestSchemaConstraints(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	const at = "2026-01-01T00:00:00Z"

	// FK: an event for a non-existent document is rejected (proves foreign_keys=ON).
	if _, err := s.perm.Exec(
		`INSERT INTO events(doc_id, session_id, edits, at) VALUES(?,?,?,?)`,
		999999, s.sessionID, `[]`, at,
	); err == nil {
		t.Error("FK not enforced: event with bad doc_id was accepted")
	}

	// CHECK kind IN ('file','scratch','chat'): an unknown kind is rejected.
	if _, err := s.perm.Exec(
		`UPDATE documents SET kind='bogus' WHERE id=?`, docID,
	); err == nil {
		t.Error("CHECK not enforced: documents.kind='bogus' was accepted")
	}

	// CHECK seq >= 0 on snapshots: a negative seq is rejected (needs a real blob
	// so the blob_hash FK is satisfied and only the seq CHECK can fail).
	hash, err := s.PutBlob("x")
	if err != nil {
		t.Fatalf("PutBlob: %v", err)
	}
	if _, err := s.perm.Exec(
		`INSERT INTO snapshots(doc_id, blob_hash, source, seq, created_at) VALUES(?,?,?,?,?)`,
		docID, hash, "local", -1, at,
	); err == nil {
		t.Error("CHECK not enforced: snapshot with seq=-1 was accepted")
	}

	// CHECK current_seq >= 0 on session_documents (v10 — moved off documents).
	if _, err := s.perm.Exec(`INSERT INTO session_documents(session_id, doc_id, current_seq) VALUES(?,?,-1)`, s.sessionID, docID); err == nil {
		t.Error("CHECK not enforced: session_documents.current_seq=-1 was accepted")
	}
}
