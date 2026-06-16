package docstate

import (
	"database/sql"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

// ---- helpers ----------------------------------------------------------------

// newStoreAtPath creates a Store backed by a real file at permPath.
// The caller is responsible for closing the store.
func newStoreAtPath(t *testing.T, permPath string) *Store {
	t.Helper()
	perm, err := openPerm(permPath)
	if err != nil {
		t.Fatalf("newStoreAtPath: openPerm %q: %v", permPath, err)
	}
	mem, err := openMem()
	if err != nil {
		perm.Close()
		t.Fatalf("newStoreAtPath: openMem: %v", err)
	}
	return &Store{perm: perm, mem: mem, clock: time.Now, currentSeq: math.MaxInt64}
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

// singleInsert returns a one-element AppliedEdit slice that looks like a
// single character insertion with no deleted text.
func singleInsert(ch string) []buffer.AppliedEdit {
	return []buffer.AppliedEdit{{Start: 0, End: 0, Deleted: "", Insert: ch}}
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

	docID, err := s.EnsureDocument("/test.md")
	if err != nil {
		t.Fatalf("EnsureDocument: %v", err)
	}

	snapID, err := s.CreateSnapshot(docID, content, "local")
	if err != nil {
		t.Fatalf("CreateSnapshot: %v", err)
	}
	if snapID <= 0 {
		t.Errorf("expected positive snapshot id, got %d", snapID)
	}

	// Assert exactly one snapshots row with correct doc_id, source.
	var gotDocID int64
	var gotSource string
	if err := s.perm.QueryRow(
		`SELECT doc_id, source FROM snapshots WHERE id=?`, snapID,
	).Scan(&gotDocID, &gotSource); err != nil {
		t.Fatalf("query snapshots: %v", err)
	}
	if gotDocID != docID {
		t.Errorf("doc_id: got %d, want %d", gotDocID, docID)
	}
	if gotSource != "local" {
		t.Errorf("source: got %q, want %q", gotSource, "local")
	}

	// LatestSnapshot reconstructs the original content.
	got, err := s.LatestSnapshot(docID)
	if err != nil {
		t.Fatalf("LatestSnapshot: %v", err)
	}
	if got != content {
		t.Errorf("LatestSnapshot: got %q, want %q", got, content)
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
		docID, err := s.EnsureDocument("/notes.md")
		if err != nil {
			t.Fatalf("EnsureDocument: %v", err)
		}
		savedDocID = docID

		edits := singleInsert("a")
		if err := s.AppendEdit("main", edits, noCursors, noCursors, "main"); err != nil {
			t.Fatalf("AppendEdit: %v", err)
		}
		if _, err := s.CreateSnapshot(docID, content, "local"); err != nil {
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

		got, err := s.LatestSnapshot(savedDocID)
		if err != nil {
			t.Fatalf("LatestSnapshot after reopen: %v", err)
		}
		if got != content {
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

	t.Setenv("HOME", noPermDir)

	store, warn, err := Open()
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
	docID, err := store.EnsureDocument("/degraded.md")
	if err != nil {
		t.Fatalf("store not functional after degradation: EnsureDocument: %v", err)
	}
	if _, err := store.CreateSnapshot(docID, "content", "local"); err != nil {
		t.Fatalf("store not functional after degradation: CreateSnapshot: %v", err)
	}
}

// ---- P1: undo/redo correctness (journal) ------------------------------------

func TestBasicUndo(t *testing.T) {
	s := NewTestStore(t)

	edits := singleInsert("a")
	before := []cursor.Cursor{{Position: 0, Anchor: 0}}
	after := []cursor.Cursor{{Position: 1, Anchor: 1}}

	if err := s.AppendEdit("main", edits, before, after, "main"); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	surface, gotEdits, gotBefore, ok := s.UndoTarget()
	if !ok {
		t.Fatal("UndoTarget: expected ok=true, got false")
	}
	if surface != "main" {
		t.Errorf("surface: got %q, want %q", surface, "main")
	}
	if len(gotEdits) != 1 || gotEdits[0].Insert != "a" {
		t.Errorf("edits: got %+v, want insert=a", gotEdits)
	}
	if len(gotBefore) != 1 || gotBefore[0].Position != 0 {
		t.Errorf("cursorsBefore: got %+v, want position=0", gotBefore)
	}

	// Second undo: nothing left.
	_, _, _, ok2 := s.UndoTarget()
	if ok2 {
		t.Error("second UndoTarget: expected ok=false (nothing left to undo)")
	}
}

func TestBasicRedo(t *testing.T) {
	s := NewTestStore(t)

	edits := singleInsert("b")
	after := []cursor.Cursor{{Position: 1, Anchor: 1}}

	if err := s.AppendEdit("main", edits, noCursors, after, "main"); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	// Undo.
	if _, _, _, ok := s.UndoTarget(); !ok {
		t.Fatal("UndoTarget returned ok=false")
	}

	// Redo.
	surface, gotEdits, gotAfter, ok := s.RedoTarget()
	if !ok {
		t.Fatal("RedoTarget: expected ok=true, got false")
	}
	if surface != "main" {
		t.Errorf("surface: got %q, want %q", surface, "main")
	}
	if len(gotEdits) != 1 || gotEdits[0].Insert != "b" {
		t.Errorf("edits: got %+v, want insert=b", gotEdits)
	}
	if len(gotAfter) != 1 || gotAfter[0].Position != 1 {
		t.Errorf("cursorsAfter: got %+v, want position=1", gotAfter)
	}

	// Second redo: nothing left.
	_, _, _, ok2 := s.RedoTarget()
	if ok2 {
		t.Error("second RedoTarget: expected ok=false (nothing left to redo)")
	}
}

func TestCoalescingWithinWindow(t *testing.T) {
	s := NewTestStore(t)

	// Fix the clock at t=0.
	now := time.Now()
	s.clock = func() time.Time { return now }

	if err := s.AppendEdit("main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Advance clock by 100ms (within 300ms window).
	now = now.Add(100 * time.Millisecond)
	if err := s.AppendEdit("main", singleInsert("b"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 2: %v", err)
	}

	n := countRows(t, s.mem, `SELECT COUNT(*) FROM events WHERE is_undo_stop=1`)
	if n != 1 {
		t.Errorf("expected 1 coalesced event row, got %d", n)
	}
}

func TestCoalescingOutsideWindow(t *testing.T) {
	s := NewTestStore(t)

	now := time.Now()
	s.clock = func() time.Time { return now }

	if err := s.AppendEdit("main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Advance clock by 400ms (outside 300ms window).
	now = now.Add(400 * time.Millisecond)
	if err := s.AppendEdit("main", singleInsert("b"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 2: %v", err)
	}

	n := countRows(t, s.mem, `SELECT COUNT(*) FROM events WHERE is_undo_stop=1`)
	if n != 2 {
		t.Errorf("expected 2 event rows (no coalesce), got %d", n)
	}
}

func TestCoalescingWhitespaceBreaks(t *testing.T) {
	s := NewTestStore(t)

	now := time.Now()
	s.clock = func() time.Time { return now }

	// First insert: non-whitespace.
	if err := s.AppendEdit("main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Second insert: space — must break coalesce.
	now = now.Add(50 * time.Millisecond)
	if err := s.AppendEdit("main", singleInsert(" "), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit space: %v", err)
	}
	// Third insert after whitespace — new stop.
	now = now.Add(50 * time.Millisecond)
	if err := s.AppendEdit("main", singleInsert("b"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 3: %v", err)
	}

	n := countRows(t, s.mem, `SELECT COUNT(*) FROM events WHERE is_undo_stop=1`)
	// Expect: 'a' (stop 1), ' ' coalesces with 'a' but wait — whitespace is
	// the LAST insert of an event, so the NEXT insert ('b') cannot coalesce with it.
	// Sequence: [a] + [space] (space coalesces into 'a' event? No — space is inserted
	// AFTER 'a', so 'a' event's last insert is 'a' (non-whitespace), space coalesces into it.
	// Then 'b' tries to coalesce: last event's last insert is ' ' (whitespace) → no coalesce.
	// Result: 2 events: [a+space] and [b].
	if n != 2 {
		t.Errorf("expected 2 events (whitespace breaks next coalesce), got %d", n)
	}
}

func TestCoalescingSurfaceBreak(t *testing.T) {
	s := NewTestStore(t)

	now := time.Now()
	s.clock = func() time.Time { return now }

	if err := s.AppendEdit("main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit main: %v", err)
	}
	// Different surface — must not coalesce.
	now = now.Add(50 * time.Millisecond)
	if err := s.AppendEdit("title", singleInsert("b"), noCursors, noCursors, "title"); err != nil {
		t.Fatalf("AppendEdit title: %v", err)
	}

	n := countRows(t, s.mem, `SELECT COUNT(*) FROM events WHERE is_undo_stop=1`)
	if n != 2 {
		t.Errorf("expected 2 events (surface break prevents coalesce), got %d", n)
	}
}

func TestTruncateOnNewEdit(t *testing.T) {
	s := NewTestStore(t)

	// Append 3 edits.
	for _, ch := range []string{"a", "b", "c"} {
		if err := s.AppendEdit("main", singleInsert(ch), noCursors, noCursors, "main"); err != nil {
			t.Fatalf("AppendEdit %q: %v", ch, err)
		}
	}

	// Since all are within the same instant (no clock advance) they may coalesce.
	// Advance clock to ensure separate stops.
	now := time.Now()
	s.clock = func() time.Time { return now }
	s2 := NewTestStore(t)
	s2.clock = s.clock

	// Use a fresh store for a controlled test.
	s3 := NewTestStore(t)
	nowT := time.Now()
	clk := func() time.Time { return nowT }
	s3.clock = clk

	for _, ch := range []string{"x", "y", "z"} {
		if err := s3.AppendEdit("main", singleInsert(ch), noCursors, noCursors, "main"); err != nil {
			t.Fatalf("AppendEdit %q: %v", ch, err)
		}
		nowT = nowT.Add(400 * time.Millisecond) // ensure separate stops
	}

	before := countRows(t, s3.mem, `SELECT COUNT(*) FROM events`)
	if before != 3 {
		t.Fatalf("expected 3 events before undo, got %d", before)
	}

	// Undo once to step back.
	if _, _, _, ok := s3.UndoTarget(); !ok {
		t.Fatal("UndoTarget returned ok=false")
	}
	// Undo again.
	if _, _, _, ok := s3.UndoTarget(); !ok {
		t.Fatal("second UndoTarget returned ok=false")
	}

	// New edit after undo — must truncate the two future events.
	nowT = nowT.Add(400 * time.Millisecond)
	if err := s3.AppendEdit("main", singleInsert("w"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit after undo: %v", err)
	}

	after := countRows(t, s3.mem, `SELECT COUNT(*) FROM events`)
	// Before undo we were at seq 3. After two undos currentSeq = 1 (at or before seq 1).
	// New edit truncates seq > currentSeq (seq 2 and 3), then inserts new event.
	// Remaining: seq 1 + new event = 2 events.
	if after != 2 {
		t.Errorf("expected 2 events after truncate-on-new-edit, got %d", after)
	}

	// Redo should now return false (future was truncated).
	_, _, _, ok := s3.RedoTarget()
	if ok {
		t.Error("RedoTarget should return ok=false after truncate-on-new-edit")
	}
}
